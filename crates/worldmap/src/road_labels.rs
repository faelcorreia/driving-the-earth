//! Road names along their lines (F17b, pure). A name is rotated to the local direction of its
//! road and repeated every `SPACING_PX` along long lines. A spot is used only where the line is
//! straight enough over the text: the chord under the text must be almost as long as the text,
//! otherwise the text would leave the road. Angles are kept upright (never more than 90 degrees
//! from the horizontal), so text reads left to right.
use std::f64::consts::{FRAC_PI_2, PI};

/// Distance along a line between two names of the same road, in pixels.
pub const SPACING_PX: f64 = 280.0;
/// Most names on one line, however long it is.
pub const MAX_PER_LINE: usize = 4;
/// Smallest chord under a name, as a fraction of the name width, to accept a spot.
pub const MIN_FIT_RATIO: f64 = 0.92;

/// A name placed on a road: its center, its upright angle (radians) and its half size before
/// rotation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Label {
    pub center: [f64; 2],
    pub angle: f64,
    pub half: [f64; 2],
}

impl Label {
    /// Axis-aligned box `[x0, y0, x1, y1]` around the rotated text, for collision checks.
    pub fn aabb(&self) -> [f64; 4] {
        let (s, c) = self.angle.sin_cos();
        let (hx, hy) = (self.half[0], self.half[1]);
        let ex = hx * c.abs() + hy * s.abs();
        let ey = hx * s.abs() + hy * c.abs();
        [
            self.center[0] - ex,
            self.center[1] - ey,
            self.center[0] + ex,
            self.center[1] + ey,
        ]
    }
}

/// Cumulative arc length at every point of `pts` (first is 0).
fn cumulative(pts: &[[f64; 2]]) -> Vec<f64> {
    let mut out = Vec::with_capacity(pts.len());
    let mut acc = 0.0;
    out.push(0.0);
    for w in pts.windows(2) {
        acc += ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
        out.push(acc);
    }
    out
}

/// Point at arc length `s` (clamped to the line) on the polyline.
fn point_at(pts: &[[f64; 2]], cum: &[f64], s: f64) -> [f64; 2] {
    let last = pts.len() - 1;
    let s = s.clamp(0.0, cum[last]);
    let i = cum.partition_point(|&c| c < s).clamp(1, last);
    let seg = cum[i] - cum[i - 1];
    let t = if seg > 0.0 {
        (s - cum[i - 1]) / seg
    } else {
        0.0
    };
    let (a, b) = (pts[i - 1], pts[i]);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// Upright angle of a direction: in (-90, 90] degrees, so the text never reads upside down.
fn upright(angle: f64) -> f64 {
    let mut a = angle;
    if a > FRAC_PI_2 {
        a -= PI;
    } else if a <= -FRAC_PI_2 {
        a += PI;
    }
    a
}

/// Places a name of width `text_w` and height `text_h` along a screen polyline. Returns no
/// labels when the line is shorter than the text or too curved at a spot. Spots are evenly
/// spread along the line, at most `MAX_PER_LINE` of them.
pub fn place(pts: &[[f64; 2]], text_w: f64, text_h: f64) -> Vec<Label> {
    if pts.len() < 2 || text_w <= 0.0 {
        return Vec::new();
    }
    let cum = cumulative(pts);
    let total = cum[cum.len() - 1];
    if total < text_w * 1.2 {
        return Vec::new();
    }
    let n = ((total / SPACING_PX).floor() as usize).clamp(1, MAX_PER_LINE);
    let mut out = Vec::with_capacity(n);
    for k in 1..=n {
        let mid = total * k as f64 / (n + 1) as f64;
        let (s0, s1) = (mid - text_w / 2.0, mid + text_w / 2.0);
        if s0 < 0.0 || s1 > total {
            continue;
        }
        let (p0, p1) = (point_at(pts, &cum, s0), point_at(pts, &cum, s1));
        let (dx, dy) = (p1[0] - p0[0], p1[1] - p0[1]);
        let chord = (dx * dx + dy * dy).sqrt();
        if chord < MIN_FIT_RATIO * text_w {
            continue;
        }
        out.push(Label {
            center: [(p0[0] + p1[0]) / 2.0, (p0[1] + p1[1]) / 2.0],
            angle: upright(dy.atan2(dx)),
            half: [text_w / 2.0, text_h / 2.0],
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(from: [f64; 2], to: [f64; 2], parts: usize) -> Vec<[f64; 2]> {
        (0..=parts)
            .map(|i| {
                let t = i as f64 / parts as f64;
                [
                    from[0] + (to[0] - from[0]) * t,
                    from[1] + (to[1] - from[1]) * t,
                ]
            })
            .collect()
    }

    #[test]
    fn short_lines_get_no_name() {
        let pts = line([0.0, 0.0], [50.0, 0.0], 4);
        assert!(place(&pts, 60.0, 12.0).is_empty());
        assert!(place(&[[0.0, 0.0]], 10.0, 12.0).is_empty());
    }

    #[test]
    fn straight_line_repeats_along_its_length() {
        let pts = line([0.0, 100.0], [1000.0, 100.0], 10);
        let labels = place(&pts, 80.0, 12.0);
        assert_eq!(labels.len(), 3);
        for l in &labels {
            assert!(l.angle.abs() < 1e-9);
            assert!((l.center[1] - 100.0).abs() < 1e-9);
        }
        assert!(labels.windows(2).all(|w| w[1].center[0] > w[0].center[0]));
    }

    #[test]
    fn medium_line_gets_fewer_names() {
        let pts = line([0.0, 0.0], [400.0, 0.0], 4);
        assert_eq!(place(&pts, 80.0, 12.0).len(), 1);
    }

    #[test]
    fn angle_follows_the_road_and_stays_upright() {
        let up_right = place(&line([0.0, 1000.0], [1000.0, 0.0], 10), 80.0, 12.0);
        assert!((up_right[0].angle + PI / 4.0).abs() < 1e-6);
        // Same road walked the other way: the text is flipped back to upright.
        let down_left = place(&line([1000.0, 0.0], [0.0, 1000.0], 10), 80.0, 12.0);
        assert!((down_left[0].angle + PI / 4.0).abs() < 1e-6);
        let left = place(&line([800.0, 100.0], [0.0, 100.0], 8), 80.0, 12.0);
        assert!(left.iter().all(|l| l.angle.abs() < 1e-9));
        let vertical = place(&line([50.0, 0.0], [50.0, 900.0], 9), 80.0, 12.0);
        assert!(vertical
            .iter()
            .all(|l| (l.angle.abs() - FRAC_PI_2).abs() < 1e-9));
    }

    #[test]
    fn curved_spots_are_skipped() {
        // A tight U-turn: each window's chord is far shorter than the text.
        let pts: Vec<[f64; 2]> = (0..=40)
            .map(|i| {
                let a = PI * i as f64 / 40.0;
                [100.0 * a.cos(), 100.0 * a.sin()]
            })
            .collect();
        let labels = place(&pts, 160.0, 12.0);
        assert!(labels.is_empty());
    }

    #[test]
    fn aabb_contains_the_rotated_text() {
        let l = Label {
            center: [10.0, 10.0],
            angle: PI / 4.0,
            half: [30.0, 6.0],
        };
        let b = l.aabb();
        let r = 30.0 * (PI / 4.0).cos().abs() + 6.0 * (PI / 4.0).sin().abs();
        assert!((b[2] - b[0] - 2.0 * r).abs() < 1e-9);
        assert!(((b[0] + b[2]) / 2.0 - 10.0).abs() < 1e-9);
        let flat = Label {
            center: [0.0, 0.0],
            angle: 0.0,
            half: [30.0, 6.0],
        };
        assert_eq!(flat.aabb(), [-30.0, -6.0, 30.0, 6.0]);
    }
}
