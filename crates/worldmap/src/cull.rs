//! Culling helpers for the home picker's drawing (F17b, pure): bounding boxes of points, boxes
//! that overlap, and long polylines cut into chunks with their own box, so that a frame transforms
//! only the pieces that reach the screen. Boxes are `[x0, y0, x1, y1]`.

/// Bounding box of points. An empty list gives an inverted box (it overlaps nothing).
pub fn bbox_of(pts: &[[f64; 2]]) -> [f64; 4] {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for p in pts {
        b = [
            b[0].min(p[0]),
            b[1].min(p[1]),
            b[2].max(p[0]),
            b[3].max(p[1]),
        ];
    }
    b
}

/// Whether two boxes touch or overlap.
pub fn boxes_overlap(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

/// Index ranges `[start, end)` that cut a polyline of `n` points into chunks of about `size`
/// points. Consecutive chunks share their end point, so the line stays connected. Chunks have at
/// least 2 points; a polyline shorter than 2 points gives none.
pub fn chunk_ranges(n: usize, size: usize) -> Vec<(usize, usize)> {
    let size = size.max(2);
    let mut out = Vec::new();
    if n < 2 {
        return out;
    }
    let mut start = 0;
    while start < n - 1 {
        let end = (start + size - 1).min(n - 1) + 1;
        out.push((start, end));
        start = end - 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbox_covers_all_points() {
        let b = bbox_of(&[[0.2, 0.9], [0.5, 0.1], [0.3, 0.4]]);
        assert_eq!(b, [0.2, 0.1, 0.5, 0.9]);
        assert!(bbox_of(&[]).iter().take(2).all(|v| *v == f64::MAX));
    }

    #[test]
    fn overlap_includes_touching_and_excludes_apart() {
        let a = [0.0, 0.0, 1.0, 1.0];
        assert!(boxes_overlap(a, [1.0, 0.5, 2.0, 2.0]));
        assert!(boxes_overlap(a, [0.2, 0.2, 0.3, 0.3]));
        assert!(!boxes_overlap(a, [1.1, 0.0, 2.0, 1.0]));
        assert!(!boxes_overlap(a, [0.0, -2.0, 1.0, -0.5]));
    }

    #[test]
    fn chunks_cover_every_segment_once() {
        for n in [0usize, 1, 2, 3, 5, 64, 65, 200] {
            let ranges = chunk_ranges(n, 64);
            let mut segments = 0;
            for (s, e) in &ranges {
                assert!(e - s >= 2 && *e <= n, "n {n} range {s}..{e}");
                segments += e - s - 1;
            }
            assert_eq!(segments, n.saturating_sub(1), "n {n}");
            if n >= 2 {
                assert_eq!(ranges[0].0, 0);
                assert_eq!(ranges.last().unwrap().1, n);
            }
            for w in ranges.windows(2) {
                assert_eq!(w[0].1 - 1, w[1].0, "shared end point, n {n}");
            }
        }
    }

    #[test]
    fn chunk_size_is_respected() {
        assert_eq!(chunk_ranges(10, 4), vec![(0, 4), (3, 7), (6, 10)]);
        assert_eq!(chunk_ranges(3, 2), vec![(0, 2), (1, 3)]);
    }
}
