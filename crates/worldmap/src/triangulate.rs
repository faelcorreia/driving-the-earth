//! Ear-clipping triangulation of one ring (F17b2). The home picker fills land and water rings
//! with triangle arrays computed here, once, at load time, so the engine never triangulates
//! at draw time (its triangulation is what logged "Invalid polygon data" for some rings).
//! A ring that cannot be triangulated (self-intersecting, degenerate) yields fewer triangles
//! and never an error: the caller just draws what came out.

/// Triangles as indices into `pts`, counter-clockwise in the input's own orientation.
/// A repeated closing point is ignored. Returns an empty list for fewer than 3 distinct points.
pub fn ear_clip(pts: &[[f64; 2]]) -> Vec<[u32; 3]> {
    let mut n = pts.len();
    if n > 1 && pts[0] == pts[n - 1] {
        n -= 1;
    }
    if n < 3 {
        return Vec::new();
    }
    let area2: f64 = (0..n)
        .map(|i| cross(pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]))
        .sum::<f64>();
    // Work on a counter-clockwise list of indices; reversed if the ring is clockwise.
    let mut idx: Vec<usize> = (0..n).collect();
    if area2 < 0.0 {
        idx.reverse();
    }
    let mut out = Vec::with_capacity(n.saturating_sub(2));
    let mut at = 0usize;
    let mut misses = 0usize;
    while idx.len() > 3 && misses < idx.len() {
        let m = idx.len();
        let i = at % m;
        let (a, b, c) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
        if is_ear(pts, &idx, i) {
            out.push([a as u32, b as u32, c as u32]);
            idx.remove(i);
            at = i;
            misses = 0;
        } else {
            at = i + 1;
            misses += 1;
        }
    }
    if idx.len() == 3 && cross(pts[idx[0]], pts[idx[1]], pts[idx[2]]) > 0.0 {
        out.push([idx[0] as u32, idx[1] as u32, idx[2] as u32]);
    }
    out
}

fn cross(o: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// Whether the corner at position `i` of `idx` is an ear: convex, and no other remaining
/// reflex vertex lies inside its triangle.
fn is_ear(pts: &[[f64; 2]], idx: &[usize], i: usize) -> bool {
    let m = idx.len();
    let (a, b, c) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
    let (pa, pb, pc) = (pts[a], pts[b], pts[c]);
    if cross(pa, pb, pc) <= 0.0 {
        return false;
    }
    for j in 0..m {
        let v = idx[j];
        if v == a || v == b || v == c {
            continue;
        }
        let p = pts[v];
        if p == pa || p == pb || p == pc {
            continue;
        }
        // Only reflex vertices can lie inside an ear; checking them keeps this cheap.
        let prev = pts[idx[(j + m - 1) % m]];
        let next = pts[idx[(j + 1) % m]];
        if cross(prev, p, next) > 0.0 {
            continue;
        }
        if cross(pa, pb, p) >= 0.0 && cross(pb, pc, p) >= 0.0 && cross(pc, pa, p) >= 0.0 {
            return false;
        }
    }
    true
}

/// Sum of the triangle areas (positive). Used by the tests and by callers that want a sanity check.
pub fn triangles_area(pts: &[[f64; 2]], tris: &[[u32; 3]]) -> f64 {
    tris.iter()
        .map(|t| cross(pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]).abs() / 2.0)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(pts: &[[f64; 2]]) -> f64 {
        let n = pts.len();
        (0..n)
            .map(|i| pts[i][0] * pts[(i + 1) % n][1] - pts[(i + 1) % n][0] * pts[i][1])
            .sum::<f64>()
            .abs()
            / 2.0
    }

    #[test]
    fn square_gives_two_triangles() {
        let sq = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let t = ear_clip(&sq);
        assert_eq!(t.len(), 2);
        assert!((triangles_area(&sq, &t) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn clockwise_and_closed_rings_work() {
        let cw = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        let t = ear_clip(&cw);
        assert_eq!(t.len(), 2);
        assert!((triangles_area(&cw, &t) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn concave_l_shape_keeps_its_area() {
        let l = [
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 1.0],
            [1.0, 1.0],
            [1.0, 2.0],
            [0.0, 2.0],
        ];
        let t = ear_clip(&l);
        assert_eq!(t.len(), 4);
        assert!((triangles_area(&l, &t) - 3.0).abs() < 1e-12);
    }

    #[test]
    fn degenerate_input_gives_nothing() {
        assert!(ear_clip(&[]).is_empty());
        assert!(ear_clip(&[[0.0, 0.0], [1.0, 1.0]]).is_empty());
        // Collinear points: no convex corner, no triangles, no panic.
        let line = [[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0]];
        assert!(ear_clip(&line).is_empty());
    }

    #[test]
    fn self_intersecting_ring_terminates() {
        // A bow-tie: must return (possibly partial) triangles without looping forever.
        let bow = [[0.0, 0.0], [1.0, 1.0], [1.0, 0.0], [0.0, 1.0]];
        let t = ear_clip(&bow);
        assert!(t.len() <= 2);
    }

    #[test]
    fn many_vertices_are_fast_enough() {
        // A circle of 2000 points: all triangles, total area close to the disc.
        let n = 2000;
        let pts: Vec<[f64; 2]> = (0..n)
            .map(|i| {
                let a = i as f64 / n as f64 * std::f64::consts::TAU;
                [a.cos(), a.sin()]
            })
            .collect();
        let t = ear_clip(&pts);
        assert_eq!(t.len(), n - 2);
        assert!((triangles_area(&pts, &t) - area(&pts)).abs() < 1e-6);
    }
}
