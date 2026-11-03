//! Clipping of OSM geometry to a tile rectangle, in the local plane (meters).
//! Polylines use Liang-Barsky, polygons use Sutherland-Hodgman.

/// Axis-aligned rectangle `[x0, x1] x [y0, y1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

type Pt = [f64; 2];

fn lerp(a: Pt, b: Pt, t: f64) -> Pt {
    [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]
}

/// Clips the segment `a-b` to `r`. `None` if it lies outside (borders included).
pub fn clip_segment(a: Pt, b: Pt, r: Rect) -> Option<(Pt, Pt)> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let checks = [
        (-dx, a[0] - r.x0),
        (dx, r.x1 - a[0]),
        (-dy, a[1] - r.y0),
        (dy, r.y1 - a[1]),
    ];
    for (p, q) in checks {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                if t > t1 {
                    return None;
                }
                t0 = t0.max(t);
            } else {
                if t < t0 {
                    return None;
                }
                t1 = t1.min(t);
            }
        }
    }
    // Keep the original endpoints bit-exact when they are not clipped: `lerp(a, b, 1.0)`
    // may differ from `b` by a rounding error, which would split a polyline at its nodes.
    let p = if t0 == 0.0 { a } else { lerp(a, b, t0) };
    let q = if t1 == 1.0 { b } else { lerp(a, b, t1) };
    Some((p, q))
}

fn push_distinct(v: &mut Vec<Pt>, p: Pt) {
    if v.last() != Some(&p) {
        v.push(p);
    }
}

/// Clips a polyline to `r`. Returns the inside pieces (each with at least 2 points).
pub fn clip_polyline(points: &[Pt], r: Rect) -> Vec<Vec<Pt>> {
    let mut out = Vec::new();
    let mut cur: Vec<Pt> = Vec::new();
    for w in points.windows(2) {
        match clip_segment(w[0], w[1], r) {
            Some((p, q)) => {
                // Entering from outside (or a new piece): flush the previous one.
                if cur.last() != Some(&p) {
                    if cur.len() >= 2 {
                        out.push(std::mem::take(&mut cur));
                    }
                    cur.clear();
                }
                push_distinct(&mut cur, p);
                push_distinct(&mut cur, q);
            }
            None => {
                if cur.len() >= 2 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
            }
        }
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
    out
}

/// One Sutherland-Hodgman pass against the half-plane `p[axis] >= bound` (`keep_ge`)
/// or `p[axis] <= bound` (otherwise).
fn clip_half(poly: &[Pt], axis: usize, bound: f64, keep_ge: bool) -> Vec<Pt> {
    let inside = |p: Pt| {
        if keep_ge {
            p[axis] >= bound
        } else {
            p[axis] <= bound
        }
    };
    let mut out = Vec::with_capacity(poly.len() + 2);
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let (ia, ib) = (inside(a), inside(b));
        if ia {
            out.push(a);
        }
        if ia != ib {
            // Only reached when a[axis] != b[axis], so the division is safe.
            let t = (bound - a[axis]) / (b[axis] - a[axis]);
            let mut p = lerp(a, b, t);
            p[axis] = bound;
            out.push(p);
        }
    }
    out
}

/// Clips a closed ring (without the repeated last point) to `r`.
/// The result may have fewer than 3 points when the polygon does not reach the rectangle.
pub fn clip_polygon(ring: &[Pt], r: Rect) -> Vec<Pt> {
    let mut poly = ring.to_vec();
    poly = clip_half(&poly, 0, r.x0, true);
    poly = clip_half(&poly, 0, r.x1, false);
    poly = clip_half(&poly, 1, r.y0, true);
    clip_half(&poly, 1, r.y1, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Rect = Rect {
        x0: 0.0,
        y0: 0.0,
        x1: 250.0,
        y1: 250.0,
    };

    #[test]
    fn segment_inside_is_unchanged() {
        let s = clip_segment([10.0, 10.0], [20.0, 30.0], R);
        assert_eq!(s, Some(([10.0, 10.0], [20.0, 30.0])));
    }

    #[test]
    fn segment_crossing_is_cut() {
        let (p, q) = clip_segment([-100.0, 125.0], [350.0, 125.0], R).unwrap();
        assert_eq!(p, [0.0, 125.0]);
        assert_eq!(q, [250.0, 125.0]);
    }

    #[test]
    fn segment_outside_is_none() {
        assert!(clip_segment([-100.0, -10.0], [-1.0, 300.0], R).is_none());
        assert!(clip_segment([300.0, 10.0], [300.0, 20.0], R).is_none());
    }

    #[test]
    fn polyline_split_in_pieces_across_tiles() {
        let road = [[-100.0, 125.0], [600.0, 125.0]];
        let inside_0 = clip_polyline(&road, R);
        assert_eq!(inside_0, vec![vec![[0.0, 125.0], [250.0, 125.0]]]);
        let r1 = Rect { x0: 250.0, x1: 500.0, ..R };
        assert_eq!(clip_polyline(&road, r1), vec![vec![[250.0, 125.0], [500.0, 125.0]]]);
        let r2 = Rect { x0: 500.0, x1: 750.0, ..R };
        assert_eq!(clip_polyline(&road, r2), vec![vec![[500.0, 125.0], [600.0, 125.0]]]);
    }

    #[test]
    fn polyline_inside_with_inexact_nodes_is_one_piece() {
        // Coordinates that are not exact in binary: nodes must not split the way.
        let line = [[10.1, 10.3], [77.7, 90.9], [200.3, 150.7], [33.3, 44.4], [12.7, 201.9]];
        let pieces = clip_polyline(&line, R);
        assert_eq!(pieces, vec![line.to_vec()]);
    }

    #[test]
    fn polyline_leaves_and_reenters() {
        // Goes in, out through the top, and back in: two pieces.
        let line = [[10.0, 10.0], [10.0, 300.0], [20.0, 300.0], [20.0, 10.0]];
        let pieces = clip_polyline(&line, R);
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[0], vec![[10.0, 10.0], [10.0, 250.0]]);
        assert_eq!(pieces[1], vec![[20.0, 250.0], [20.0, 10.0]]);
    }

    #[test]
    fn polygon_half_inside_keeps_half_area() {
        // 100x100 square, centered on the right border x = 250: half inside.
        let sq = [[200.0, 50.0], [300.0, 50.0], [300.0, 150.0], [200.0, 150.0]];
        let out = clip_polygon(&sq, R);
        assert_eq!(out.len(), 4);
        let area = shoelace(&out);
        assert!((area - 50.0 * 100.0).abs() < 1e-9, "{area}");
    }

    #[test]
    fn polygon_outside_is_empty() {
        let sq = [[300.0, 300.0], [400.0, 300.0], [400.0, 400.0], [300.0, 400.0]];
        assert!(clip_polygon(&sq, R).len() < 3);
    }

    fn shoelace(p: &[Pt]) -> f64 {
        let n = p.len();
        let s: f64 = (0..n)
            .map(|i| {
                let (a, b) = (p[i], p[(i + 1) % n]);
                a[0] * b[1] - b[0] * a[1]
            })
            .sum();
        (s / 2.0).abs()
    }
}
