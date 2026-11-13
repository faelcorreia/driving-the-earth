//! Vertical profile of a road: the final terrain sampled along the whole way, smoothed in
//! arc length, capped at a little above the ground, and pinned to one shared height at the
//! nodes where ways meet (or end). The cap is measured against the real terrain under each
//! point (the same surface the game draws), because the along-way smoothing overshoots the
//! ground on a convex break. The shared height comes from a smoothed 2-D terrain field at the
//! node, so every way through a node gets the same value. Each way's profile is then bent to
//! that value with a fade whose length grows with the change. Horizontal geometry is never
//! changed.
use std::collections::{HashMap, HashSet};

/// Spacing of the samples along the way (m).
pub const STEP_M: f64 = 2.5;
/// Gaussian sigma of the smoothing along the way (m).
pub const SIGMA_M: f64 = 12.0;
/// The road may rise at most this far above the terrain (m).
pub const CAP_ABOVE_M: f64 = 0.5;
/// Shortest length over which a pinned node's correction fades out (m).
const RAMP_MIN_M: f64 = 20.0;
/// The fade grows with the correction: this many metres of fade per metre of change.
const RAMP_PER_M: f64 = 100.0;
/// The road may sit at most this far below the smoothed terrain (m).
pub const FLOOR_BELOW_M: f64 = 0.15;
/// Transverse slope of the road edges is limited to this (rise over run).
pub const SLOPE_LIMIT: f64 = 0.12;
/// Spacing of the samples of the 2-D terrain field (m).
const FIELD_STEP_M: f64 = 5.0;

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
        let a = if det.abs() > 1e-12 * w0 * w0 {
            (w2 * v0 - w1 * v1) / det
        } else {
            v0 / w0
        };
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
    /// Smoothed terrain at each point (the floor reference: raw DEM noise must not reach the road).
    pub t: Vec<f64>,
    /// Real terrain at each point, on the same triangles the game draws (the cap reference).
    pub raw: Vec<f64>,
    /// Smoothed height at each point, capped at the real terrain plus `CAP_ABOVE_M`.
    pub z: Vec<f64>,
}

/// Keeps each height at most `CAP_ABOVE_M` above the real terrain of the same point.
fn cap_to(z: &mut [f64], raw: &[f64]) {
    for (zi, ri) in z.iter_mut().zip(raw) {
        *zi = zi.min(ri + CAP_ABOVE_M);
    }
}

/// Keeps a height between `t - FLOOR_BELOW_M` and the ground cap `raw + CAP_ABOVE_M`. When the
/// floor and the cap do not fit (steep, rough ground), the cap wins: a road may dip below the
/// smoothed terrain but never float above the surface the game draws.
fn clamp_to_band(z: f64, t: f64, raw: f64) -> f64 {
    let hi = raw + CAP_ABOVE_M;
    z.clamp((t - FLOOR_BELOW_M).min(hi), hi)
}

/// First pass: the way densified and smoothed in arc length over the final terrain.
/// `terrain` gives the final terrain height at a point (None if unknown: no profile).
pub fn smooth_way(
    points: &[[f64; 2]],
    terrain: &dyn Fn([f64; 2]) -> Option<f64>,
) -> Option<SmoothWay> {
    let pts = densify(points);
    if pts.len() < 2 {
        return None;
    }
    let s = arc_lengths(&pts);
    let raw: Vec<f64> = pts.iter().map(|&p| terrain(p)).collect::<Option<_>>()?;
    let t = gauss(&raw, &s, SIGMA_M);
    let mut z = t.clone();
    cap_to(&mut z, &raw);
    let mut z = gauss(&z, &s, SIGMA_M);
    cap_to(&mut z, &raw);
    Some(SmoothWay { pts, s, t, raw, z })
}

/// Smoothed terrain at `p`: the Gaussian-weighted mean of the terrain on a 2-D grid around
/// `p` (sigma `SIGMA_M`, reach 3 sigma). It depends on the point only, never on the way, so a
/// node gets one value wherever it is used. `None` when no terrain is known around `p`.
pub fn field(p: [f64; 2], terrain: &dyn Fn([f64; 2]) -> Option<f64>) -> Option<f64> {
    let reach = 3.0 * SIGMA_M;
    let n = (reach / FIELD_STEP_M).ceil() as i32;
    let (mut w0, mut v0) = (0.0, 0.0);
    for j in -n..=n {
        for i in -n..=n {
            let (dx, dy) = (i as f64 * FIELD_STEP_M, j as f64 * FIELD_STEP_M);
            let d2 = dx * dx + dy * dy;
            if d2 > reach * reach {
                continue;
            }
            if let Some(h) = terrain([p[0] + dx, p[1] + dy]) {
                let w = (-d2 / (2.0 * SIGMA_M * SIGMA_M)).exp();
                w0 += w;
                v0 += w * h;
            }
        }
    }
    (w0 > 0.0).then(|| v0 / w0)
}

/// Shared height of each pinned node: one value for all the ways that meet there. It is the
/// smoothed terrain field at the node, kept inside the band that every incident way allows
/// (`[t - FLOOR_BELOW_M, min(t, raw) + CAP_ABOVE_M]`, `t` = that way's smoothed terrain and
/// `raw` = the real terrain at the node), so the cap and the floor hold for all of them at
/// once. When the band is empty the cap wins: the node sinks to the tightest upper bound.
pub fn node_heights(
    ways: &[Option<SmoothWay>],
    pins: &HashSet<(i64, i64)>,
    terrain: &dyn Fn([f64; 2]) -> Option<f64>,
) -> HashMap<(i64, i64), f64> {
    struct Node {
        p: [f64; 2],
        lo: f64,
        hi: f64,
        t_sum: f64,
        n: u32,
    }
    let mut acc: HashMap<(i64, i64), Node> = HashMap::new();
    for w in ways.iter().flatten() {
        for k in 0..w.pts.len() {
            let key = node_key(w.pts[k]);
            if pins.contains(&key) {
                let t = w.t[k];
                let raw = w.raw[k];
                let e = acc.entry(key).or_insert(Node {
                    p: w.pts[k],
                    lo: f64::MIN,
                    hi: f64::MAX,
                    t_sum: 0.0,
                    n: 0,
                });
                e.lo = e.lo.max(t - FLOOR_BELOW_M);
                e.hi = e.hi.min(t.min(raw) + CAP_ABOVE_M);
                e.t_sum += t;
                e.n += 1;
            }
        }
    }
    acc.into_iter()
        .map(|(k, nd)| {
            let want = field(nd.p, terrain).unwrap_or(nd.t_sum / nd.n as f64);
            let h = if nd.lo <= nd.hi {
                want.clamp(nd.lo, nd.hi)
            } else {
                nd.hi
            };
            (k, h)
        })
        .collect()
}

/// Second pass: heights of one way, one value per point of `sw.pts`. Each pinned point takes
/// the shared node height. The change at a pin fades out with a smoothstep over a length of
/// `max(RAMP_MIN_M, RAMP_PER_M * |change|)`, never past the neighbouring pin. The result is
/// kept inside the band of the smoothed terrain (floor) and of the real terrain (cap); pins are
/// set last and exactly, already inside that band by construction. The cap wins over the floor.
pub fn pin_way(sw: &SmoothWay, node_h: &HashMap<(i64, i64), f64>) -> Vec<f32> {
    let pins: Vec<(usize, f64)> = (0..sw.pts.len())
        .filter_map(|i| node_h.get(&node_key(sw.pts[i])).map(|&h| (i, h)))
        .collect();
    let mut z = fade_way(sw, node_h);
    if !pins.is_empty() {
        for (i, zi) in z.iter_mut().enumerate() {
            *zi = clamp_to_band(*zi, sw.t[i], sw.raw[i]);
        }
        for &(k, h) in &pins {
            z[k] = h;
        }
    }
    z.into_iter().map(|v| v as f32).collect()
}

/// The fade of `pin_way` before the band: pins at their shared height, the change between
/// them faded out with a smoothstep. Exposed so the fade can be checked on its own.
pub fn fade_way(sw: &SmoothWay, node_h: &HashMap<(i64, i64), f64>) -> Vec<f64> {
    let pins: Vec<(usize, f64)> = (0..sw.pts.len())
        .filter_map(|i| node_h.get(&node_key(sw.pts[i])).map(|&h| (i, h)))
        .collect();
    let mut z = sw.z.clone();
    if !pins.is_empty() {
        let delta: Vec<f64> = pins.iter().map(|&(k, h)| h - sw.z[k]).collect();
        let ramp: Vec<f64> = pins
            .iter()
            .enumerate()
            .map(|(n, &(k, _))| {
                let mut len = (RAMP_PER_M * delta[n].abs()).max(RAMP_MIN_M);
                if n > 0 {
                    len = len.min(sw.s[k] - sw.s[pins[n - 1].0]);
                }
                if n + 1 < pins.len() {
                    len = len.min(sw.s[pins[n + 1].0] - sw.s[k]);
                }
                len
            })
            .collect();
        for (i, zi) in z.iter_mut().enumerate() {
            let mut corr = 0.0;
            for (n, &(k, _)) in pins.iter().enumerate() {
                let d = (sw.s[i] - sw.s[k]).abs();
                if d < ramp[n] {
                    corr += delta[n] * smoothstep(1.0 - d / ramp[n]);
                }
            }
            *zi += corr;
        }
        for &(k, h) in &pins {
            z[k] = h;
        }
    }
    z
}

/// Heights of the two edges at each point of a densified way: `z` minus and plus half the
/// width times the transverse slope of the smoothed terrain (rising to the right = positive).
/// The slope is sampled at the edges and limited to `SLOPE_LIMIT`; with no terrain around the
/// edges the cross-section stays flat.
pub fn edge_heights(
    pts: &[[f64; 2]],
    z: &[f32],
    half_w: f64,
    terrain: &dyn Fn([f64; 2]) -> Option<f64>,
) -> (Vec<f32>, Vec<f32>) {
    let n = pts.len();
    let mut zl = Vec::with_capacity(n);
    let mut zr = Vec::with_capacity(n);
    for i in 0..n {
        let (a, b) = (pts[i.saturating_sub(1)], pts[(i + 1).min(n - 1)]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = dx.hypot(dy);
        let slope = if len > 1e-9 && half_w > 0.0 {
            let (r, p) = ([dy / len, -dx / len], pts[i]);
            let right = field([p[0] + r[0] * half_w, p[1] + r[1] * half_w], terrain);
            let left = field([p[0] - r[0] * half_w, p[1] - r[1] * half_w], terrain);
            match (right, left) {
                (Some(tr), Some(tl)) => {
                    ((tr - tl) / (2.0 * half_w)).clamp(-SLOPE_LIMIT, SLOPE_LIMIT)
                }
                _ => 0.0,
            }
        } else {
            0.0
        };
        let off = half_w * slope;
        zl.push((z[i] as f64 - off) as f32);
        zr.push((z[i] as f64 + off) as f32);
    }
    (zl, zr)
}

/// Heights of the pieces of `orig` clipped to a tile. `piece` points lie on `orig` (clipping
/// only drops points and adds crossings), so each takes the height of the nearest segment of
/// `orig`, interpolated by its position along that segment.
pub fn z_for_piece(orig: &[[f64; 2]], z: &[f32], piece: &[[f64; 2]]) -> Vec<f32> {
    piece.iter().map(|&p| z_at(orig, z, p)).collect()
}

/// Caps the centerline of a clipped piece at `CAP_ABOVE_M` above the terrain the tile draws,
/// moving the two edges by the same amount so the cross-section is kept. Clipping adds a point
/// at every tile border by interpolating `z` (`z_for_piece`); on a concave break of the terrain
/// at the border (each tile grid has its own slope there), that chord can float above the
/// ground even though both ends are capped. The cap is the same on both sides because
/// neighbouring tiles share their edge heights.
pub fn cap_piece(
    piece: &[[f64; 2]],
    z: &mut [f32],
    zl: &mut [f32],
    zr: &mut [f32],
    terrain: &dyn Fn([f64; 2]) -> Option<f64>,
) {
    for (i, p) in piece.iter().enumerate() {
        let Some(raw) = terrain(*p) else { continue };
        let Some(zi) = z.get_mut(i) else { continue };
        let capped = (*zi as f64).min(raw + CAP_ABOVE_M);
        let delta = capped - *zi as f64;
        *zi = capped as f32;
        if let Some(v) = zl.get_mut(i) {
            *v = (*v as f64 + delta) as f32;
        }
        if let Some(v) = zr.get_mut(i) {
            *v = (*v as f64 + delta) as f32;
        }
    }
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
    count
        .into_iter()
        .filter(|(_, c)| *c >= 2)
        .map(|(k, _)| k)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn densify_keeps_vertices_and_limits_the_step() {
        let d = densify(&[[0.0, 0.0], [10.0, 0.0], [10.0, 1.0]]);
        assert!(d.contains(&[10.0, 0.0]) && d[0] == [0.0, 0.0] && d.last() == Some(&[10.0, 1.0]));
        for w in d.windows(2) {
            assert!(
                ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt() <= STEP_M + 1e-9
            );
        }
    }

    #[test]
    fn smoothing_removes_a_zigzag_and_keeps_a_ramp() {
        let s: Vec<f64> = (0..200).map(|i| i as f64 * 2.5).collect();
        let zig: Vec<f64> = (0..200)
            .map(|i| if i % 2 == 0 { 0.25 } else { -0.25 })
            .collect();
        let out = gauss(&zig, &s, SIGMA_M);
        assert!(out[100].abs() < 0.01, "{}", out[100]);
        let ramp: Vec<f64> = s.iter().map(|x| 0.05 * x).collect();
        let out = gauss(&ramp, &s, SIGMA_M);
        assert!((out[100] - ramp[100]).abs() < 1e-6);
        assert!(
            (out[0] - ramp[0]).abs() < 1e-6 && (out[199] - ramp[199]).abs() < 1e-6,
            "ends"
        );
    }

    #[test]
    fn profile_pins_nodes_and_caps_above_terrain() {
        // Terrain: a crest at x = 100 m. Smoothing would lower the crest below the terrain,
        // so the height must stay on the smoothed line; the pinned end takes the shared height.
        let terrain = |p: [f64; 2]| Some(10.0 + (-(p[0] - 100.0).powi(2) / 800.0).exp() * 3.0);
        let line = [[0.0, 0.0], [200.0, 0.0]];
        let sw = smooth_way(&line, &terrain).unwrap();
        let pins = HashSet::from([node_key([0.0, 0.0])]);
        let h = node_heights(&[Some(sw)], &pins, &terrain);
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
    fn profile_does_not_float_over_a_convex_break() {
        // 30% descent for 30 m, then flat. Smoothing along the way overshoots the ground just
        // after the break: capping against the smoothed `t` keeps the road almost 2 m above
        // the real terrain at the break, which is what the drawn surface shows. The cap must
        // use the real terrain, so every point stays within CAP_ABOVE_M of it.
        let ground = |p: [f64; 2]| {
            Some(if p[0] <= 30.0 {
                20.0 - 0.30 * p[0]
            } else {
                11.0
            })
        };
        let line = [[0.0, 0.0], [100.0, 0.0]];
        let ways = vec![Some(smooth_way(&line, &ground).unwrap())];
        let pins = pinned_nodes(&[line.to_vec()]);
        let h = node_heights(&ways, &pins, &ground);
        let sw = ways[0].as_ref().unwrap();
        let z = pin_way(sw, &h);
        let mut worst = f64::MIN;
        for (zi, p) in z.iter().zip(&sw.pts) {
            let above = *zi as f64 - ground(*p).unwrap();
            worst = worst.max(above);
            assert!(
                above <= CAP_ABOVE_M + 1e-4,
                "{above:.2} m above the ground at {p:?}"
            );
        }
        assert!(worst > 0.0, "the break is never reached: {worst}");
    }

    #[test]
    fn ways_meeting_at_a_node_share_its_height() {
        // A noisy terrain: the raw height at the node is a spike; the shared height must not be it.
        let spike = |p: [f64; 2]| Some(10.0 + if p == [50.0, 0.0] { 2.0 } else { 0.0 });
        let a = smooth_way(&[[0.0, 0.0], [50.0, 0.0], [100.0, 0.0]], &spike).unwrap();
        let b = smooth_way(&[[50.0, 0.0], [50.0, 50.0]], &spike).unwrap();
        let pins = pinned_nodes(&[
            vec![[0.0, 0.0], [50.0, 0.0], [100.0, 0.0]],
            vec![[50.0, 0.0], [50.0, 50.0]],
        ]);
        let h = node_heights(&[Some(a), Some(b)], &pins, &spike);
        let node = node_key([50.0, 0.0]);
        assert!(h[&node] < 10.6, "{}", h[&node]);
        let a = smooth_way(&[[0.0, 0.0], [50.0, 0.0], [100.0, 0.0]], &spike).unwrap();
        let za = pin_way(&a, &h);
        let i = a.pts.iter().position(|&p| node_key(p) == node).unwrap();
        assert!((za[i] as f64 - h[&node]).abs() < 1e-6);
    }

    /// Two ways meeting at (50, 0): one along x, one north. A bump next to the north way makes
    /// their smoothed terrain differ at the node.
    fn junction_case() -> (Vec<SmoothWay>, HashMap<(i64, i64), f64>, (i64, i64)) {
        let bump = |p: [f64; 2]| {
            Some(10.0 + 1.5 * (-((p[0] - 50.0).powi(2) + (p[1] - 25.0).powi(2)) / 200.0).exp())
        };
        let a = vec![[0.0, 0.0], [50.0, 0.0], [100.0, 0.0]];
        let b = vec![[50.0, 0.0], [50.0, 50.0]];
        let pins = pinned_nodes(&[a.clone(), b.clone()]);
        let ways = [smooth_way(&a, &bump), smooth_way(&b, &bump)];
        let node = node_key([50.0, 0.0]);
        let h = node_heights(&ways, &pins, &bump);
        (ways.into_iter().flatten().collect(), h, node)
    }

    #[test]
    fn shared_node_has_one_height_in_every_way() {
        let (ways, h, node) = junction_case();
        let mut zs = Vec::new();
        for sw in &ways {
            let z = pin_way(sw, &h);
            let i = sw.pts.iter().position(|&p| node_key(p) == node).unwrap();
            assert!(
                (z[i] as f64 - h[&node]).abs() < 1e-5,
                "pin is the shared height"
            );
            assert!(
                h[&node] >= sw.t[i] - FLOOR_BELOW_M - 1e-9
                    && h[&node] <= sw.t[i] + CAP_ABOVE_M + 1e-9
            );
            zs.push(z[i]);
        }
        assert!(zs.windows(2).all(|w| w[0] == w[1]), "{zs:?}");
    }

    #[test]
    fn height_changes_between_stations_are_bounded() {
        let (ways, h, _) = junction_case();
        for sw in &ways {
            let z = fade_way(sw, &h);
            let corr: Vec<f64> = z.iter().zip(&sw.z).map(|(&zi, &si)| zi - si).collect();
            let max_d = sw
                .pts
                .iter()
                .enumerate()
                .filter_map(|(i, &p)| h.get(&node_key(p)).map(|&v| v - sw.z[i]))
                .fold(0.0, |m: f64, d| m.max(d.abs()));
            let bound = 1.5 * STEP_M * max_d / RAMP_MIN_M + 1e-6;
            for w in corr.windows(2) {
                assert!(
                    (w[1] - w[0]).abs() <= bound,
                    "{} > {bound}",
                    (w[1] - w[0]).abs()
                );
            }
            for (zi, ti) in pin_way(sw, &h).iter().zip(&sw.t) {
                assert!(
                    (*zi as f64) >= ti - FLOOR_BELOW_M - 1e-4
                        && (*zi as f64) <= ti + CAP_ABOVE_M + 1e-4
                );
            }
        }
    }

    #[test]
    fn edges_follow_the_transverse_slope() {
        // Terrain rises to the north (+y); a road going east has its left edge on the north side.
        let pts: Vec<[f64; 2]> = (0..40).map(|i| [i as f64 * 2.5, 0.0]).collect();
        let z = vec![10.0f32; pts.len()];
        let (zl, zr) = edge_heights(&pts, &z, 3.0, &|p: [f64; 2]| Some(10.0 + 0.1 * p[1]));
        let mid = pts.len() / 2;
        assert!(
            (zl[mid] - 10.3).abs() < 1e-3 && (zr[mid] - 9.7).abs() < 1e-3,
            "{} {}",
            zl[mid],
            zr[mid]
        );
        let (zl, zr) = edge_heights(&pts, &z, 3.0, &|p: [f64; 2]| Some(10.0 + 0.5 * p[1]));
        let limit = 3.0 * SLOPE_LIMIT;
        assert!(
            (zl[mid] as f64 - (10.0 + limit)).abs() < 1e-3
                && (zr[mid] as f64 - (10.0 - limit)).abs() < 1e-3
        );
    }

    #[test]
    fn z_follows_the_clipped_piece() {
        let orig = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]];
        let z = [0.0f32, 1.0, 3.0];
        let zs = z_for_piece(&orig, &z, &[[5.0, 0.0], [10.0, 5.0]]);
        assert!(
            (zs[0] - 0.5).abs() < 1e-6 && (zs[1] - 2.0).abs() < 1e-6,
            "{zs:?}"
        );
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
