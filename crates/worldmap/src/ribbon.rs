//! Road ribbons (F17b, pure): a polyline in screen pixels turned into a triangle list of a given
//! width, so a tile's roads can be built once and drawn as one mesh instead of one draw call per
//! road. Each segment is a quad; each inner corner gets a miter (its tip at most `MITER_LIMIT`
//! half-widths from the corner) or a bevel beyond that. Ends are butt-cut. Corners that reverse
//! (a 180 degree turn) get no join, which leaves the two quads overlapping.
use std::f64;

/// Miter tip distance from the corner, in half-widths, above which the join is beveled.
pub const MITER_LIMIT: f64 = 2.0;

/// Vertices (pixels) and triangle indices of any number of ribbons.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Ribbon {
    pub verts: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

fn unit(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = (dx * dx + dy * dy).sqrt();
    [dx / l, dy / l]
}

fn perp(d: [f64; 2]) -> [f64; 2] {
    [-d[1], d[0]]
}

impl Ribbon {
    /// Appends the ribbon of `pts` (screen pixels) with width `width` (pixels). Consecutive repeated
    /// points are skipped; a line with fewer than two distinct points, or a width of 0, adds nothing.
    pub fn push_polyline(&mut self, pts: &[[f64; 2]], width: f64) {
        let hw = width / 2.0;
        if !(hw > 0.0) {
            return;
        }
        let mut p: Vec<[f64; 2]> = Vec::with_capacity(pts.len());
        for &q in pts {
            if p.last() != Some(&q) {
                p.push(q);
            }
        }
        if p.len() < 2 {
            return;
        }
        let dirs: Vec<[f64; 2]> = p.windows(2).map(|w| unit(w[0], w[1])).collect();
        for (i, w) in p.windows(2).enumerate() {
            let n = perp(dirs[i]);
            let (dx, dy) = (n[0] * hw, n[1] * hw);
            let base = self.verts.len() as u32;
            for q in [
                [w[0][0] + dx, w[0][1] + dy],
                [w[1][0] + dx, w[1][1] + dy],
                [w[1][0] - dx, w[1][1] - dy],
                [w[0][0] - dx, w[0][1] - dy],
            ] {
                self.verts.push([q[0] as f32, q[1] as f32]);
            }
            self.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        for i in 1..p.len() - 1 {
            let (d0, d1) = (dirs[i - 1], dirs[i]);
            let cross = d0[0] * d1[1] - d0[1] * d1[0];
            let k = 1.0 + d0[0] * d1[0] + d0[1] * d1[1];
            if cross.abs() < 1e-12 || k <= 1e-12 {
                continue;
            }
            // The outer side of the corner is opposite to the turn.
            let s = if cross > 0.0 { -1.0 } else { 1.0 };
            let (n0, n1) = (perp(d0), perp(d1));
            let c = p[i];
            let a = [c[0] + s * n0[0] * hw, c[1] + s * n0[1] * hw];
            let b = [c[0] + s * n1[0] * hw, c[1] + s * n1[1] * hw];
            let off = [s * (n0[0] + n1[0]) * hw / k, s * (n0[1] + n1[1]) * hw / k];
            let tip = [c[0] + off[0], c[1] + off[1]];
            let base = self.verts.len() as u32;
            let tip_len = (off[0] * off[0] + off[1] * off[1]).sqrt();
            if tip_len <= MITER_LIMIT * hw {
                for q in [c, a, tip, b] {
                    self.verts.push([q[0] as f32, q[1] as f32]);
                }
                self.indices.extend_from_slice(&[
                    base,
                    base + 1,
                    base + 2,
                    base,
                    base + 2,
                    base + 3,
                ]);
            } else {
                for q in [c, a, b] {
                    self.verts.push([q[0] as f32, q[1] as f32]);
                }
                self.indices.extend_from_slice(&[base, base + 1, base + 2]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_segment_is_one_quad_of_the_width() {
        let mut r = Ribbon::default();
        r.push_polyline(&[[0.0, 0.0], [10.0, 0.0]], 4.0);
        assert_eq!(
            r.verts,
            vec![[0.0, 2.0], [10.0, 2.0], [10.0, -2.0], [0.0, -2.0]]
        );
        assert_eq!(r.indices, vec![0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn right_angle_gets_a_miter_tip() {
        let mut r = Ribbon::default();
        r.push_polyline(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]], 2.0);
        // Two quads (8 vertices) and one miter join (4 vertices).
        assert_eq!(r.verts.len(), 12);
        assert_eq!(r.indices.len(), 6 + 6 + 6);
        assert!(r.verts.contains(&[11.0, -1.0]), "{:?}", r.verts);
    }

    #[test]
    fn sharp_spike_is_beveled_not_mitered() {
        let mut r = Ribbon::default();
        r.push_polyline(&[[0.0, 0.0], [10.0, 0.0], [0.0, 1.0]], 2.0);
        // Two quads (8 vertices) and one bevel (3 vertices): no far miter tip.
        assert_eq!(r.verts.len(), 11);
        assert_eq!(r.indices.len(), 6 + 6 + 3);
        assert!(
            r.verts.iter().all(|v| v[0] <= 10.0 + 1.0 && v[0] >= -1.0),
            "{:?}",
            r.verts
        );
    }

    #[test]
    fn degenerate_input_adds_nothing_and_repeats_are_skipped() {
        let mut r = Ribbon::default();
        r.push_polyline(&[[1.0, 1.0]], 2.0);
        r.push_polyline(&[[1.0, 1.0], [1.0, 1.0]], 2.0);
        r.push_polyline(&[[0.0, 0.0], [5.0, 0.0]], 0.0);
        assert!(r.verts.is_empty() && r.indices.is_empty());
        r.push_polyline(&[[0.0, 0.0], [0.0, 0.0], [5.0, 0.0]], 2.0);
        assert_eq!(r.verts.len(), 4);
    }

    #[test]
    fn all_vertices_are_finite_and_indices_in_range() {
        let mut r = Ribbon::default();
        let pts = [
            [0.0, 0.0],
            [3.0, 4.0],
            [3.0, 4.0],
            [-2.0, 7.0],
            [-2.0, -1.0],
            [0.0, 0.0],
        ];
        r.push_polyline(&pts, 3.5);
        assert!(r.verts.iter().all(|v| v[0].is_finite() && v[1].is_finite()));
        assert!(r.indices.iter().all(|&i| (i as usize) < r.verts.len()));
        assert_eq!(r.indices.len() % 3, 0);
    }
}

/// Width in tile-local units of a road drawn `px_width` pixels wide at zoom step `step` (see
/// `labels::zoom_step`) in a tile at zoom `z`. A tile's items are drawn with scale
/// `px_per_unit / 2^z`, so the drawn width is `px_width * px_per_unit / 2^step`: nominal at the
/// start of the step and about double at its end.
pub fn local_width(px_width: f64, step: i32, z: u8) -> f64 {
    px_width * 2f64.powi(z as i32 - step)
}

#[cfg(test)]
mod local_width_tests {
    use super::*;

    #[test]
    fn width_is_nominal_at_step_start_and_about_double_at_step_end() {
        for (step, z) in [(8, 10u8), (9, 14), (3, 2)] {
            let drawn = |ppu: f64| local_width(2.0, step, z) * ppu / 2f64.powi(z as i32);
            let start = 2f64.powi(step);
            assert!((drawn(start) - 2.0).abs() < 1e-12);
            assert!((drawn(start * 1.5) - 3.0).abs() < 1e-12);
            assert!((drawn(start * 2.0 * (1.0 - 1e-9)) - 4.0).abs() < 1e-6);
        }
    }
}
