//! Distant lights (L2), pure part (no Godot): reads `cache/<id>/far_lights.bin` on a background
//! thread, keeps at most `max` points by weight, and builds the MultiMesh buffer the render layer
//! hands to Godot in one call. A missing file is not an error: there are no distant lights.
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Instant;

use tileformat::far_lights::{read_far_lights, FarLight};

use crate::environment::lamps::far_delay_s;

/// Floats per instance of the MultiMesh buffer (TRANSFORM_3D with custom data): 12 of transform
/// (three rows of four, the origin last in each) and 4 of custom data.
pub const FLOATS_PER_INSTANCE: usize = 16;

/// What the loader thread hands back.
pub struct FarLoad {
    /// MultiMesh buffer, `FLOATS_PER_INSTANCE` per light.
    pub buffer: Vec<f32>,
    /// Delay of each light in seconds, sorted, to count the lit ones with `lit_count`.
    pub delays: Vec<f32>,
    /// Points in the file and points kept after the cut by weight.
    pub file_points: usize,
    pub loaded_points: usize,
    /// Read, cut and built, in seconds.
    pub secs: f64,
}

/// Godot position of a distant light: map `x`, height `z`, and map `y` turned into `-z`, as the
/// tiles and the lamps do.
pub fn godot_position(p: &FarLight) -> [f32; 3] {
    [p.x, p.z, -p.y]
}

/// The lights kept under `max`: all of them when there are no more than `max`, otherwise the
/// `max` heaviest (ties by file order). Returns each light with its index in the file, in file
/// order, because the index gives its delay.
pub fn select_by_weight(points: &[FarLight], max: usize) -> Vec<(u32, FarLight)> {
    let mut order: Vec<usize> = (0..points.len()).collect();
    if points.len() > max {
        order.sort_by(|&a, &b| points[b].weight.cmp(&points[a].weight).then(a.cmp(&b)));
        order.truncate(max);
        order.sort_unstable();
    }
    order.into_iter().map(|i| (i as u32, points[i])).collect()
}

/// Smoothstep of the distance `d` (m) across the handover band centred on `swap_m`: 0 nearer than
/// `swap_m - band_m / 2` (the real street lamp takes over), 1 farther than `swap_m + band_m / 2`
/// (the distant light shows). The shader computes this; the test pins the formula.
#[cfg(test)]
pub fn handover_fade(d: f64, swap_m: f64, band_m: f64) -> f64 {
    let t = ((d - (swap_m - band_m / 2.0)) / band_m).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Radius of a light on screen, in pixels, with a soft floor: `sqrt(r^2 + min^2)` is about `r` for
/// large lights and about `min` for sub-pixel ones, so the size never jumps across the floor.
/// The shader computes this; the test pins the formula.
#[cfg(test)]
pub fn soft_min_radius_px(radius_px: f64, min_px: f64) -> f64 {
    (radius_px * radius_px + min_px * min_px).sqrt()
}

/// The MultiMesh buffer of the kept lights and their sorted delays (seconds).
pub fn build(points: &[(u32, FarLight)]) -> (Vec<f32>, Vec<f32>) {
    let mut buffer = Vec::with_capacity(points.len() * FLOATS_PER_INSTANCE);
    let mut delays = Vec::with_capacity(points.len());
    for &(index, p) in points {
        let [x, y, z] = godot_position(&p);
        // Identity basis, origin last in each row.
        buffer.extend_from_slice(&[1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, y, 0.0, 0.0, 1.0, z]);
        let delay = far_delay_s(index as u64) as f32;
        // Custom data read by the shader: delay (s), hue and weight in 0..1.
        buffer.extend_from_slice(&[delay, p.hue as f32 / 255.0, p.weight as f32 / 255.0, 0.0]);
        delays.push(delay);
    }
    delays.sort_by(|a, b| a.total_cmp(b));
    (buffer, delays)
}

/// Number of lights lit `elapsed_s` seconds after the street lamps came on, from sorted delays.
pub fn lit_count(sorted_delays: &[f32], elapsed_s: f64) -> usize {
    sorted_delays.partition_point(|&d| (d as f64) <= elapsed_s)
}

/// Reads and builds the file in the background. The receiver gets one result: `NotFound` when
/// the map has no `far_lights.bin`.
pub fn spawn_load(path: PathBuf, max: usize) -> Receiver<io::Result<FarLoad>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let t0 = Instant::now();
        let result = read_far_lights(&path).map(|points| {
            let file_points = points.len();
            let kept = select_by_weight(&points, max);
            let (buffer, delays) = build(&kept);
            FarLoad {
                buffer,
                delays,
                file_points,
                loaded_points: kept.len(),
                secs: t0.elapsed().as_secs_f64(),
            }
        });
        let _ = tx.send(result);
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light(x: f32, y: f32, z: f32, weight: u8, hue: u8) -> FarLight {
        FarLight {
            x,
            y,
            z,
            weight,
            hue,
        }
    }

    #[test]
    fn map_y_becomes_negative_godot_z() {
        assert_eq!(
            godot_position(&light(1.0, 2.0, 3.0, 0, 0)),
            [1.0, 3.0, -2.0]
        );
        assert_eq!(
            godot_position(&light(-5.5, -7.0, 0.5, 0, 0)),
            [-5.5, 0.5, 7.0]
        );
    }

    #[test]
    fn cut_keeps_the_heaviest_in_file_order() {
        let pts = [
            light(0.0, 0.0, 0.0, 1, 0),
            light(1.0, 0.0, 0.0, 9, 0),
            light(2.0, 0.0, 0.0, 9, 0),
            light(3.0, 0.0, 0.0, 2, 0),
            light(4.0, 0.0, 0.0, 5, 0),
        ];
        let kept: Vec<u32> = select_by_weight(&pts, 3).iter().map(|k| k.0).collect();
        assert_eq!(kept, vec![1, 2, 4]);
    }

    #[test]
    fn no_cut_when_within_the_maximum() {
        let pts = [light(0.0, 0.0, 0.0, 1, 0), light(1.0, 0.0, 0.0, 2, 0)];
        assert_eq!(select_by_weight(&pts, 2).len(), 2);
        assert_eq!(select_by_weight(&pts, 10).len(), 2);
        assert!(select_by_weight(&[], 0).is_empty());
    }

    #[test]
    fn buffer_has_origin_last_and_custom_data() {
        let kept = [(7u32, light(10.0, 20.0, 30.0, 255, 128))];
        let (buffer, delays) = build(&kept);
        assert_eq!(buffer.len(), FLOATS_PER_INSTANCE);
        assert_eq!(&buffer[0..3], &[1.0, 0.0, 0.0]);
        assert_eq!((buffer[3], buffer[7], buffer[11]), (10.0, 30.0, -20.0));
        assert_eq!(buffer[12], far_delay_s(7) as f32);
        assert!((buffer[13] - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(buffer[14], 1.0);
        assert_eq!(delays, vec![far_delay_s(7) as f32]);
    }

    #[test]
    fn handover_fade_is_zero_near_and_one_far_within_the_band() {
        assert_eq!(handover_fade(500.0, 600.0, 20.0), 0.0);
        assert_eq!(handover_fade(590.0, 600.0, 20.0), 0.0);
        assert_eq!(handover_fade(610.0, 600.0, 20.0), 1.0);
        assert_eq!(handover_fade(900.0, 600.0, 20.0), 1.0);
        assert!((handover_fade(600.0, 600.0, 20.0) - 0.5).abs() < 1e-12);
        let mut last = 0.0;
        for i in 0..=200 {
            let f = handover_fade(590.0 + i as f64 * 0.1, 600.0, 20.0);
            assert!(f >= last, "not monotonic at step {i}");
            last = f;
        }
    }

    #[test]
    fn soft_min_radius_keeps_large_lights_and_floors_small_ones() {
        assert!((soft_min_radius_px(100.0, 1.25) - 100.0).abs() < 0.01);
        assert!((soft_min_radius_px(0.0, 1.25) - 1.25).abs() < 1e-12);
        let r = soft_min_radius_px(0.5, 1.25);
        assert!(r > 1.25 && r < 1.35, "{r}");
    }

    #[test]
    fn lit_count_follows_the_delays() {
        let delays = [0.0f32, 60.0, 60.0, 300.0];
        assert_eq!(lit_count(&delays, -1.0), 0);
        assert_eq!(lit_count(&delays, 0.0), 1);
        assert_eq!(lit_count(&delays, 60.0), 3);
        assert_eq!(lit_count(&delays, 1000.0), 4);
    }
}
