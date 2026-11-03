//! 2D vector helpers on `[f64; 2]` points, used by road and junction geometry.
//! Pure Rust (no Godot types). Imports nothing from this crate.

/// Miter joins are cut back when the corner is sharper than this (scale limit).
const MITER_LIMIT: f64 = 2.0;

/// Unit vector from `p` to `q`. Caller ensures `p != q`.
pub(super) fn unit(p: [f64; 2], q: [f64; 2]) -> [f64; 2] {
    let len = dist(p, q);
    [(q[0] - p[0]) / len, (q[1] - p[1]) / len]
}

/// Distance from `p` to the segment `a -> b`.
pub(super) fn dist_to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    dist(p, [a[0] + t * dx, a[1] + t * dy])
}

pub(super) fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
}

pub(super) fn scale(v: [f64; 2], k: f64) -> [f64; 2] {
    [v[0] * k, v[1] * k]
}

/// Unit normal (left of the direction) of the segment `p -> q`. Caller ensures `p != q`.
pub(super) fn unit_normal(p: [f64; 2], q: [f64; 2]) -> [f64; 2] {
    let len = dist(p, q);
    [-(q[1] - p[1]) / len, (q[0] - p[0]) / len]
}

/// Offset of length `half` at a node joining segments with normals `m1` and `m2`:
/// the bisector scaled so both edges stay at `half` from the centerline.
pub(super) fn miter(m1: [f64; 2], m2: [f64; 2], half: f64) -> [f64; 2] {
    let sum = [m1[0] + m2[0], m1[1] + m2[1]];
    let len = (sum[0] * sum[0] + sum[1] * sum[1]).sqrt();
    if len < 1e-9 {
        // Opposite directions (U-turn): no bisector, keep the first normal.
        return scale(m1, half);
    }
    let bisector = scale(sum, 1.0 / len);
    let cos_half = (bisector[0] * m1[0] + bisector[1] * m1[1]).max(1.0 / MITER_LIMIT);
    scale(bisector, half / cos_half)
}
