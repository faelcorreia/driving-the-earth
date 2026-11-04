//! Runtime smoothing of the longitudinal profile of a road (S1 of the terrain revamp). Pure Rust.
//!
//! The bake gives each road point a height sampled from the DEM. Over that noise, the profile is
//! rebuilt as straight ramps of constant grade joined by vertical parabolas, whose length is
//! `L = K * A` (A: grade change in percent; K: DNIT IPR-740 Tab. 66 for the design speed of the
//! class, convex for crests, concave for sags), rounded up to whole `CURVE_STEP_M`.
//!
//! Rules: the profile keeps its endpoints (so tiles that clip one way meet at the same height);
//! a piece steeper than the class ceiling is real grade and is kept as sampled; a ramp is never
//! flattened to a ceiling; a curve that does not fit between its neighbors is left out (the corner
//! stays sharp) rather than shortened below K.
//!
//! Output: the smoothed heights at the same points, plus the edges shifted by the same amount, so
//! the transverse slope of the road is kept.
use tileformat::units::profile::{CURVE_STEP_M, MIN_RAMP_M, NOISE_TOL_M};

/// Smallest change of grade (percent) that makes a corner; smaller changes are merged away.
const CURVE_MIN_CHANGE_PCT: f64 = 0.2;

/// Largest vertical distance (m) between a merged ramp and any sample it covers.
const MERGE_MAX_DEV_M: f64 = 0.3;

/// Class of a road, from its OSM `highway` value. Decides the design speed and the grade ceiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RoadClass {
    Arterial,
    Collector,
    Local,
    Service,
}

impl RoadClass {
    pub(crate) fn of(highway: &str) -> RoadClass {
        match highway {
            "motorway" | "trunk" | "primary" => RoadClass::Arterial,
            "secondary" | "tertiary" => RoadClass::Collector,
            "residential" | "unclassified" | "living_street" => RoadClass::Local,
            _ => RoadClass::Service,
        }
    }

    /// Design speed of the class, in km/h.
    pub(crate) fn speed_kmh(self) -> u32 {
        match self {
            RoadClass::Arterial => 60,
            RoadClass::Collector => 50,
            RoadClass::Local => 40,
            RoadClass::Service => 30,
        }
    }

    /// Grade ceiling of the class, in percent: noise is smoothed below it, real grade above it is kept.
    fn ceiling_pct(self) -> f64 {
        match self {
            RoadClass::Arterial => 7.0,
            RoadClass::Collector => 8.0,
            RoadClass::Local | RoadClass::Service => 11.0,
        }
    }
}

/// K of DNIT IPR-740 Tab. 66 for a design speed: `(convex, concave)`, in meters per percent.
pub(crate) fn k_for_speed(kmh: u32) -> (f64, f64) {
    match kmh {
        0..=30 => (2.0, 6.0),
        31..=40 => (4.0, 9.0),
        41..=50 => (7.0, 13.0),
        _ => (11.0, 18.0),
    }
}

/// One piece of the smoothed profile: `z(x) = z0 + g0 * (x - x0) + c * (x - x0)^2` on `[x0, x1]`.
struct Piece {
    x0: f64,
    x1: f64,
    z0: f64,
    g0: f64,
    c: f64,
}

impl Piece {
    fn at(&self, x: f64) -> f64 {
        let d = x - self.x0;
        self.z0 + self.g0 * d + self.c * d * d
    }
}

/// Smoothed profile of one road. `points` and `z` are the bake's; `zl` and `zr` (edge heights,
/// may be empty) move by the same amount as `z`. Roads with fewer than three points, or with
/// repeated points, come back unchanged.
pub(crate) fn smooth_profile(
    points: &[[f64; 2]],
    z: &[f32],
    zl: &[f32],
    zr: &[f32],
    highway: &str,
) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let n = points.len();
    let unchanged = || (z.to_vec(), zl.to_vec(), zr.to_vec());
    if n < 3 || z.len() != n {
        return unchanged();
    }
    let s = arc_lengths(points);
    if s.windows(2).any(|w| w[1] - w[0] <= 1e-9) {
        return unchanged();
    }
    let zf: Vec<f64> = z.iter().map(|&v| v as f64).collect();
    let class = RoadClass::of(highway);
    let slope = |a: usize, b: usize| (zf[b] - zf[a]) / (s[b] - s[a]);

    // Vertices of the design, and per piece whether it is real steep grade (kept as sampled).
    let kept = simplify(&s, &zf, NOISE_TOL_M);
    let mut v: Vec<usize> = Vec::new();
    let mut raw: Vec<bool> = Vec::new();
    for w in kept.windows(2) {
        let (a, b) = (w[0], w[1]);
        // Only a piece long enough to be a ramp can be real grade; shorter steep ones are sampling noise.
        if slope(a, b).abs() * 100.0 > class.ceiling_pct() && s[b] - s[a] >= MIN_RAMP_M {
            for i in a..b {
                v.push(i);
                raw.push(true);
            }
        } else {
            v.push(a);
            raw.push(false);
        }
    }
    v.push(n - 1);

    merge_short_and_flat(&s, &zf, &mut v, &mut raw, &slope);

    // Half-length of the curve at each vertex (0 at the ends and at corners that cannot take a curve).
    let (convex, concave) = k_for_speed(class.speed_kmh());
    let last = v.len() - 1;
    let mut h = vec![0.0f64; v.len()];
    for k in 1..last {
        if raw[k - 1] || raw[k] {
            continue;
        }
        let a_pct = (slope(v[k], v[k + 1]) - slope(v[k - 1], v[k])) * 100.0;
        if a_pct.abs() < CURVE_MIN_CHANGE_PCT {
            continue;
        }
        // Grade rising (sag) takes the concave K; grade falling (crest) the convex one.
        let k_m_per_pct = if a_pct > 0.0 { concave } else { convex };
        let len = (k_m_per_pct * a_pct.abs() / CURVE_STEP_M).ceil() * CURVE_STEP_M;
        h[k] = len / 2.0;
        // A curve must fit in the piece before it (with that piece's other curve) and in the piece after it.
        let (before, after) = (s[v[k]] - s[v[k - 1]], s[v[k + 1]] - s[v[k]]);
        if h[k - 1] + h[k] > before || h[k] > after {
            h[k] = 0.0;
        }
    }

    let pieces = build_pieces(&s, &zf, &v, &h, &slope);
    let new_z: Vec<f64> = s
        .iter()
        .enumerate()
        .map(
            |(i, &x)| match pieces.iter().find(|p| x >= p.x0 - 1e-9 && x <= p.x1 + 1e-9) {
                Some(p) => p.at(x),
                None => zf[i],
            },
        )
        .collect();

    let shift = |edge: &[f32]| -> Vec<f32> {
        if edge.len() != n {
            return edge.to_vec();
        }
        (0..n)
            .map(|i| (edge[i] as f64 + new_z[i] - zf[i]) as f32)
            .collect()
    };
    (
        new_z.iter().map(|&v| v as f32).collect(),
        shift(zl),
        shift(zr),
    )
}

/// Cumulative arc length of a polyline, in meters.
fn arc_lengths(points: &[[f64; 2]]) -> Vec<f64> {
    let mut s = vec![0.0];
    for w in points.windows(2) {
        let d = (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
        s.push(s[s.len() - 1] + d);
    }
    s
}

/// Douglas-Peucker on the profile: indices of the kept points, first and last included. Each
/// dropped point lies within `tol` (vertical) of the straight piece that replaces it.
fn simplify(s: &[f64], z: &[f64], tol: f64) -> Vec<usize> {
    let n = s.len();
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (mut worst, mut at) = (0.0f64, a);
        for i in a + 1..b {
            let t = (s[i] - s[a]) / (s[b] - s[a]);
            let dev = (z[i] - (z[a] + (z[b] - z[a]) * t)).abs();
            if dev > worst {
                worst = dev;
                at = i;
            }
        }
        if worst > tol {
            keep[at] = true;
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    (0..n).filter(|&i| keep[i]).collect()
}

/// Drops interior vertices between two smooth pieces when one of them is shorter than
/// `MIN_RAMP_M` or when the grade does not change: the pieces join into one ramp. Pieces marked
/// `raw` (real steep grade) are never joined. `raw[k]` is the flag of the piece that starts at `v[k]`.
/// A join only happens when the resulting chord stays within `MERGE_MAX_DEV_M` of every sample it covers.
fn merge_short_and_flat(
    s: &[f64],
    z: &[f64],
    v: &mut Vec<usize>,
    raw: &mut Vec<bool>,
    slope: &dyn Fn(usize, usize) -> f64,
) {
    loop {
        let mut changed = false;
        let mut k = 1;
        while k + 1 < v.len() {
            if !raw[k - 1] && !raw[k] {
                let la = s[v[k]] - s[v[k - 1]];
                let lb = s[v[k + 1]] - s[v[k]];
                let (ga, gb) = (slope(v[k - 1], v[k]), slope(v[k], v[k + 1]));
                if (la < MIN_RAMP_M
                    || lb < MIN_RAMP_M
                    || (gb - ga).abs() * 100.0 < CURVE_MIN_CHANGE_PCT)
                    && chord_within(s, z, v[k - 1], v[k + 1], MERGE_MAX_DEV_M)
                {
                    v.remove(k);
                    raw.remove(k);
                    changed = true;
                    continue;
                }
            }
            k += 1;
        }
        if !changed {
            break;
        }
    }
}

/// True when the straight chord from sample `a` to sample `b` lies within `tol` (vertical) of
/// every sample from `a` to `b`.
fn chord_within(s: &[f64], z: &[f64], a: usize, b: usize, tol: f64) -> bool {
    (a + 1..b).all(|i| {
        let t = (s[i] - s[a]) / (s[b] - s[a]);
        (z[i] - (z[a] + (z[b] - z[a]) * t)).abs() <= tol
    })
}

/// The pieces of the design: a straight ramp between consecutive vertices, trimmed by the curve
/// at each end, and the parabola of each curve.
fn build_pieces(
    s: &[f64],
    z: &[f64],
    v: &[usize],
    h: &[f64],
    slope: &dyn Fn(usize, usize) -> f64,
) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let last = v.len() - 1;
    for k in 0..last {
        let (ia, ib) = (v[k], v[k + 1]);
        let g = slope(ia, ib);
        let x0 = s[ia] + h[k];
        let x1 = s[ib] - h[k + 1];
        pieces.push(Piece {
            x0,
            x1,
            z0: z[ia] + g * (x0 - s[ia]),
            g0: g,
            c: 0.0,
        });
        if k + 1 < last && h[k + 1] > 0.0 {
            let hh = h[k + 1];
            let gn = slope(ib, v[k + 2]);
            // Length 2 * hh; the parabola starts on the incoming ramp, at x1.
            pieces.push(Piece {
                x0: x1,
                x1: s[ib] + hh,
                z0: z[ib] - g * hh,
                g0: g,
                c: (gn - g) / (4.0 * hh),
            });
        }
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(
        x0: f64,
        x1: f64,
        step: f64,
        y: f64,
        f: impl Fn(f64) -> f64,
    ) -> (Vec<[f64; 2]>, Vec<f32>) {
        let mut pts = Vec::new();
        let mut z = Vec::new();
        let mut x = x0;
        while x <= x1 + 1e-9 {
            pts.push([x, y]);
            z.push(f(x) as f32);
            x += step;
        }
        (pts, z)
    }

    fn at_station(pts: &[[f64; 2]], z: &[f32], x: f64) -> f64 {
        let i = pts
            .iter()
            .position(|p| (p[0] - x).abs() < 1e-6)
            .expect("station");
        z[i] as f64
    }

    #[test]
    fn constant_ramp_is_kept() {
        let (pts, z) = line(0.0, 130.0, 2.5, 0.0, |x| 0.0769 * x);
        let (out, _, _) = smooth_profile(&pts, &z, &[], &[], "residential");
        for (a, b) in z.iter().zip(&out) {
            assert!((a - b).abs() < 1e-4, "{a} != {b}");
        }
    }

    #[test]
    fn noise_is_removed() {
        // Flat road with +-0.1 m zigzag every 2.5 m: the sampled noise is within the tolerance.
        let (pts, z) = line(0.0, 200.0, 2.5, 0.0, |x| {
            if (x / 2.5).round() as i64 % 2 == 0 {
                0.1
            } else {
                -0.1
            }
        });
        let (out, _, _) = smooth_profile(&pts, &z, &[], &[], "residential");
        let hi = out.iter().fold(f32::MIN, |m, &v| m.max(v));
        let lo = out.iter().fold(f32::MAX, |m, &v| m.min(v));
        assert!(hi - lo < 1e-3, "zigzag left in the profile: {lo}..{hi}");
    }

    #[test]
    fn real_grade_above_the_ceiling_is_kept() {
        // 15% over 100 m on a local road (ceiling 11%): kept as sampled.
        let (pts, z) = line(0.0, 100.0, 2.5, 0.0, |x| 0.15 * x);
        let (out, _, _) = smooth_profile(&pts, &z, &[], &[], "residential");
        assert_eq!(out, z);
    }

    #[test]
    fn crest_gets_a_vertical_curve_with_k_of_the_class() {
        // Primary (arterial, 60 km/h, K convex 11): grade 5% then -5% over 100 m each.
        // A = -10%, L = 11 * 10 = 110 m, rounded up to 120 m: tangents at 40 m and 160 m.
        let (pts, z) = line(0.0, 200.0, 2.5, 0.0, |x| {
            if x <= 100.0 {
                0.05 * x
            } else {
                5.0 - 0.05 * (x - 100.0)
            }
        });
        let (out, _, _) = smooth_profile(&pts, &z, &[], &[], "primary");
        assert!(
            (at_station(&pts, &out, 40.0) - 2.0).abs() < 0.01,
            "tangent at 40 m: {} {:?}",
            at_station(&pts, &out, 40.0),
            &out[..20]
        );
        assert!(
            (at_station(&pts, &out, 160.0) - 2.0).abs() < 0.01,
            "tangent at 160 m"
        );
        // Apex deviation A * L / 800 = -1.5 m, so the crest is at 3.5 m.
        assert!(
            (at_station(&pts, &out, 100.0) - 3.5).abs() < 0.02,
            "apex {}",
            at_station(&pts, &out, 100.0)
        );
        assert!(
            (at_station(&pts, &out, 20.0) - 1.0).abs() < 0.01,
            "ramp kept before the curve"
        );
    }

    #[test]
    fn concave_profile_stays_within_0_3_m_of_the_samples() {
        // Flat for 75 m, then steeper and steeper for 75 m (a sag): the smoothed profile must not
        // stray more than 0.3 m from the samples it replaces.
        let (pts, z) = line(0.0, 150.0, 2.5, 0.0, |x| {
            if x <= 75.0 {
                0.0
            } else {
                0.002 * (x - 75.0) * (x - 75.0)
            }
        });
        let (out, _, _) = smooth_profile(&pts, &z, &[], &[], "residential");
        let worst = z
            .iter()
            .zip(&out)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst <= 0.3, "profile strays {worst} m from the samples");
    }

    #[test]
    fn profile_keeps_endpoints_and_edge_shift() {
        let (pts, z) = line(0.0, 200.0, 2.5, 0.0, |x| 0.2 * (x / 20.0).sin() * 3.0);
        let zl: Vec<f32> = z.iter().map(|v| v + 0.5).collect();
        let (out, out_l, _) = smooth_profile(&pts, &z, &zl, &[], "primary");
        assert!((out[0] - z[0]).abs() < 1e-5 && (out[out.len() - 1] - z[z.len() - 1]).abs() < 1e-5);
        for i in 0..out.len() {
            assert!(
                (out_l[i] - out[i] - 0.5).abs() < 1e-4,
                "edge keeps the transverse slope at {i}"
            );
        }
    }
}
