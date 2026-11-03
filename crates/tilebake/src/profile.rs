//! Vertical profile of a road: the final terrain sampled along the whole way, smoothed in
//! arc length, capped at a little above the ground, and pinned to one shared height at the
//! nodes where ways meet (or end). Horizontal geometry is never changed.
use std::collections::{HashMap, HashSet};

/// Spacing of the samples along the way (m).
pub const STEP_M: f64 = 2.5;
/// Gaussian sigma of the smoothing along the way (m).
pub const SIGMA_M: f64 = 12.0;
/// The road may rise at most this far above the terrain (m).
pub const CAP_ABOVE_M: f64 = 0.5;
/// Length over which a pinned node's correction fades out (m).
const RAMP_M: f64 = 20.0;

/// Key used to find nodes shared by several ways: coordinates rounded to 1 cm.
pub fn node_key(p: [f64; 2]) -> (i64, i64) {
    ((p[0] * 100.0).round() as i64, (p[1] * 100.0).round() as i64)
}

/// Points of the way with extra points so that no segment is longer than `STEP_M`.
/// The original vertices are kept, so pins and clipping still see them.
pub fn densify(points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut out = Vec::with_capacity(points.len());
    for w in points.windows(2) {
        let (p, q) = (w[0], w[1]);
        out.push(p);
        let len = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt();
        let pieces = (len / STEP_M).ceil().max(1.0) as usize;
        for k in 1..pieces {
            let t = k as f64 / pieces as f64;
            out.push([p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t]);
        }
    }
    if let Some(&last) = points.last() {
        out.push(last);
    }
    out
}

/// Arc length at each point.
pub fn arc_lengths(points: &[[f64; 2]]) -> Vec<f64> {
    let mut s = Vec::with_capacity(points.len());
    let mut acc = 0.0;
    for (i, p) in points.iter().enumerate() {
        if i > 0 {
            let q = points[i - 1];
            acc += ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt();
        }
        s.push(acc);
    }
    s
}

/// Gaussian-weighted local linear fit over arc length, evaluated at each sample. A plain
/// weighted mean is biased at the ends of a way on a slope (one-sided window), so each sample
/// is the centre of a weighted least-squares line; a ramp is kept exactly, ends included.
pub fn gauss(values: &[f64], s: &[f64], sigma: f64) -> Vec<f64> {
    let reach = 3.0 * sigma;
    let mut out = Vec::with_capacity(values.len());
    let mut lo = 0;
    for i in 0..values.len() {
        while lo < i && s[i] - s[lo] > reach {
            lo += 1;
        }
        let (mut w0, mut w1, mut w2, mut v0, mut v1) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for j in lo..values.len() {
            let d = s[j] - s[i];
            if d > reach {
                break;
            }
            let w = (-(d * d) / (2.0 * sigma * sigma)).exp();
            w0 += w;
            w1 += w * d;
            w2 += w * d * d;
            v0 += w * values[j];
            v1 += w * d * values[j];
        }
        // Weighted LS of v = a + b d; the centre value is a. Degenerate (one sample): the mean.
        let det = w0 * w2 - w1 * w1;
        let a = if det.abs() > 1e-12 * w0 * w0 { (w2 * v0 - w1 * v1) / det } else { v0 / w0 };
        out.push(a);
    }
    out
}

fn smoothstep(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// A way densified and smoothed over the terrain, before the pins are applied.
pub struct SmoothWay {
    /// Densified points (see `densify`).
    pub pts: Vec<[f64; 2]>,
    /// Arc length at each point.
    pub s: Vec<f64>,
    /// Smoothed terrain at each point (the cap reference: raw DEM noise must not reach the road).
    pub t: Vec<f64>,
    /// Smoothed height at each point, capped at the smoothed terrain plus `CAP_ABOVE_M`.
    pub z: Vec<f64>,
}

/// Keeps each height at most `CAP_ABOVE_M` above the terrain of the same point.
fn cap_to(z: &mut [f64], t: &[f64]) {
    for (zi, ti) in z.iter_mut().zip(t) {
        *zi = zi.min(ti + CAP_ABOVE_M);
    }
}

/// First pass: the way densified and smoothed in arc length over the final terrain.
/// `terrain` gives the final terrain height at a point (None if unknown: no profile).
pub fn smooth_way(points: &[[f64; 2]], terrain: &dyn Fn([f64; 2]) -> Option<f64>) -> Option<SmoothWay> {
    let pts = densify(points);
    if pts.len() < 2 {
        return None;
    }
    let s = arc_lengths(&pts);
    let raw: Vec<f64> = pts.iter().map(|&p| terrain(p)).collect::<Option<_>>()?;
    let t = gauss(&raw, &s, SIGMA_M);
    let mut z = t.clone();
    cap_to(&mut z, &t);
    let mut z = gauss(&z, &s, SIGMA_M);
    cap_to(&mut z, &t);
    Some(SmoothWay { pts, s, t, z })
}

/// Shared height of each pinned node: the mean of the smoothed heights of the ways that meet
/// there (never the raw terrain, which carries DEM noise). A way whose cap is lower clamps its
/// own pin in `pin_way`, so the node stays within the terrain cap wherever it is smooth.
pub fn node_heights(
    ways: &[Option<SmoothWay>],
    pins: &HashSet<(i64, i64)>,
) -> HashMap<(i64, i64), f64> {
    let mut acc: HashMap<(i64, i64), (f64, u32)> = HashMap::new();
    for w in ways.iter().flatten() {
        for k in 0..w.pts.len() {
            let key = node_key(w.pts[k]);
            if pins.contains(&key) {
                let e = acc.entry(key).or_insert((0.0, 0));
                e.0 += w.z[k];
                e.1 += 1;
            }
        }
    }
    acc.into_iter().map(|(k, (sum, n))| (k, sum / n as f64)).collect()
}

/// Second pass: heights of one way, one value per point of `sw.pts`. Pinned points take the
/// shared node height; the correction fades out over `RAMP_M` on each side of the pin.
pub fn pin_way(sw: &SmoothWay, node_h: &HashMap<(i64, i64), f64>) -> Vec<f32> {
    let pins: Vec<(usize, f64)> = (0..sw.pts.len())
        .filter_map(|i| node_h.get(&node_key(sw.pts[i])).map(|&h| (i, h)))
        .collect();
    let mut z = sw.z.clone();
    if !pins.is_empty() {
        let delta: Vec<f64> = pins.iter().map(|&(k, h)| h - sw.z[k]).collect();
        for (i, zi) in z.iter_mut().enumerate() {
            let mut corr = 0.0;
            for (n, &(k, _)) in pins.iter().enumerate() {
                let d = (sw.s[i] - sw.s[k]).abs();
                if d < RAMP_M {
                    corr += delta[n] * smoothstep(1.0 - d / RAMP_M);
                }
            }
            *zi += corr;
        }
        for &(k, h) in &pins {
            z[k] = h;
        }
        cap_to(&mut z, &sw.t);
    }
    z.into_iter().map(|v| v as f32).collect()
}

/// Heights of the pieces of `orig` clipped to a tile. `piece` points lie on `orig` (clipping
/// only drops points and adds crossings), so each takes the height of the nearest segment of
/// `orig`, interpolated by its position along that segment.
pub fn z_for_piece(orig: &[[f64; 2]], z: &[f32], piece: &[[f64; 2]]) -> Vec<f32> {
    piece.iter().map(|&p| z_at(orig, z, p)).collect()
}

/// Height of the polyline `orig` (with heights `z`) nearest to `p`.
pub fn z_at(orig: &[[f64; 2]], z: &[f32], p: [f64; 2]) -> f32 {
    let mut best = (f64::MAX, z[0]);
    for k in 0..orig.len().saturating_sub(1) {
        let (a, b) = (orig[k], orig[k + 1]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len2 = dx * dx + dy * dy;
        let t = if len2 > 0.0 {
            (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (qx, qy) = (a[0] + dx * t, a[1] + dy * t);
        let d2 = (p[0] - qx).powi(2) + (p[1] - qy).powi(2);
        if d2 < best.0 {
            best = (d2, z[k] + (z[k + 1] - z[k]) * t as f32);
        }
    }
    best.1
}

/// Count of ways per node, and the nodes that must be pinned: shared by two or more ways, or
/// the end of a way.
pub fn pinned_nodes(ways: &[Vec<[f64; 2]>]) -> HashSet<(i64, i64)> {
    let mut count: HashMap<(i64, i64), u32> = HashMap::new();
    for w in ways {
        let Some((first, last)) = w.first().zip(w.last()) else {
            continue;
        };
        for p in [*first, *last] {
            *count.entry(node_key(p)).or_insert(0) += 2;
        }
        for p in w.iter().skip(1).take(w.len().saturating_sub(2)) {
            *count.entry(node_key(*p)).or_insert(0) += 1;
        }
    }
    count.into_iter().filter(|(_, c)| *c >= 2).map(|(k, _)| k).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn densify_keeps_vertices_and_limits_the_step() {
        let d = densify(&[[0.0, 0.0], [10.0, 0.0], [10.0, 1.0]]);
        assert!(d.contains(&[10.0, 0.0]) && d[0] == [0.0, 0.0] && d.last() == Some(&[10.0, 1.0]));
        for w in d.windows(2) {
            assert!(((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt() <= STEP_M + 1e-9);
        }
    }

    #[test]
    fn smoothing_removes_a_zigzag_and_keeps_a_ramp() {
        let s: Vec<f64> = (0..200).map(|i| i as f64 * 2.5).collect();
        let zig: Vec<f64> = (0..200).map(|i| if i % 2 == 0 { 0.25 } else { -0.25 }).collect();
        let out = gauss(&zig, &s, SIGMA_M);
        assert!(out[100].abs() < 0.01, "{}", out[100]);
        let ramp: Vec<f64> = s.iter().map(|x| 0.05 * x).collect();
        let out = gauss(&ramp, &s, SIGMA_M);
        assert!((out[100] - ramp[100]).abs() < 1e-6);
        assert!((out[0] - ramp[0]).abs() < 1e-6 && (out[199] - ramp[199]).abs() < 1e-6, "ends");
    }

    #[test]
    fn profile_pins_nodes_and_caps_above_terrain() {
        // Terrain: a crest at x = 100 m. Smoothing would lower the crest below the terrain,
        // so the height must stay on the smoothed line; the pinned end takes the shared height.
        let terrain = |p: [f64; 2]| Some(10.0 + (-(p[0] - 100.0).powi(2) / 800.0).exp() * 3.0);
        let line = [[0.0, 0.0], [200.0, 0.0]];
        let sw = smooth_way(&line, &terrain).unwrap();
        let pins = HashSet::from([node_key([0.0, 0.0])]);
        let h = node_heights(&[Some(sw)], &pins);
        let sw = smooth_way(&line, &terrain).unwrap();
        let z = pin_way(&sw, &h);
        assert!((z[0] as f64 - h[&node_key([0.0, 0.0])]).abs() < 1e-6);
        for (zi, p) in z.iter().zip(&sw.pts) {
            assert!((*zi as f64) <= terrain(*p).unwrap() + CAP_ABOVE_M + 1e-6);
        }
        let flat = smooth_way(&line, &|_| Some(10.0)).unwrap();
        let flat_z = pin_way(&flat, &HashMap::new());
        assert!(flat_z.iter().all(|v| (v - 10.0).abs() < 1e-4));
    }

    #[test]
    fn ways_meeting_at_a_node_share_its_height() {
        // A noisy terrain: the raw height at the node is a spike; the shared height must not be it.
        let spike = |p: [f64; 2]| Some(10.0 + if p == [50.0, 0.0] { 2.0 } else { 0.0 });
        let a = smooth_way(&[[0.0, 0.0], [50.0, 0.0], [100.0, 0.0]], &spike).unwrap();
        let b = smooth_way(&[[50.0, 0.0], [50.0, 50.0]], &spike).unwrap();
        let pins = pinned_nodes(&[vec![[0.0, 0.0], [50.0, 0.0], [100.0, 0.0]], vec![[50.0, 0.0], [50.0, 50.0]]]);
        let h = node_heights(&[Some(a), Some(b)], &pins);
        let node = node_key([50.0, 0.0]);
        assert!(h[&node] < 10.6, "{}", h[&node]);
        let a = smooth_way(&[[0.0, 0.0], [50.0, 0.0], [100.0, 0.0]], &spike).unwrap();
        let za = pin_way(&a, &h);
        let i = a.pts.iter().position(|&p| node_key(p) == node).unwrap();
        assert!((za[i] as f64 - h[&node]).abs() < 1e-6);
    }

    #[test]
    fn z_follows_the_clipped_piece() {
        let orig = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]];
        let z = [0.0f32, 1.0, 3.0];
        let zs = z_for_piece(&orig, &z, &[[5.0, 0.0], [10.0, 5.0]]);
        assert!((zs[0] - 0.5).abs() < 1e-6 && (zs[1] - 2.0).abs() < 1e-6, "{zs:?}");
    }

    #[test]
    fn shared_nodes_and_ends_are_pinned() {
        let a = vec![[0.0, 0.0], [50.0, 0.0], [100.0, 0.0]];
        let b = vec![[50.0, 0.0], [50.0, 50.0]];
        let pins = pinned_nodes(&[a, b]);
        assert!(pins.contains(&node_key([50.0, 0.0])));
        assert!(pins.contains(&node_key([50.0, 50.0])), "way end");
        assert!(pins.contains(&node_key([100.0, 0.0])), "way end");
        assert_eq!(pins.len(), 4);
    }
}
