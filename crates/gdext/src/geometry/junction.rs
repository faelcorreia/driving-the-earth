//! Junction geometry: extends road ends so ribbons meet at crossings and overlap across tile
//! borders. Pure Rust (no Godot types). Imports `geometry::vec2` and `tileformat`/`tilebake` data.
use tileformat::{road_width_m, Polyline};

use super::vec2::{dist, dist_to_segment, unit, unit_normal};

/// How far from another road's band an end still counts as a junction, in meters.
const JUNCTION_TOL_M: f64 = 1.0;
/// How far a road end on a tile border runs past it, in meters, so the ribbons of the two
/// tiles overlap and no hairline of terrain shows through the seam.
pub(super) const BORDER_OVERLAP_M: f64 = 0.25;

/// Points of every road of a tile, with each end that meets another road extended so that
/// its cut lies past the near edge of the crossed road. Ends on a tile border run a
/// little past it (overlap with the next tile); ends with no other road close by
/// (cul-de-sacs) are kept as they are.
/// Points of each road (extended ends included). Heights come from the terrain field, not here.
pub(super) fn extend_junction_ends(roads: &[Polyline], tile_m: f64) -> Vec<Vec<[f64; 2]>> {
    roads
        .iter()
        .enumerate()
        .map(|(i, road)| {
            let half = road_width_m(&road.highway) / 2.0;
            let mut pts = road.points.clone();
            if pts.len() < 2 {
                return pts;
            }
            let head = extend_end(&pts, half, roads, i, tile_m);
            extend_tail(&mut pts, head);
            pts.reverse();
            let tail = extend_end(&pts, half, roads, i, tile_m);
            extend_tail(&mut pts, tail);
            pts.reverse();
            pts
        })
        .collect()
}

/// Moves the last point of `pts` by `ext` meters along its direction (adds a point).
fn extend_tail(pts: &mut Vec<[f64; 2]>, ext: f64) {
    if ext <= 0.0 {
        return;
    }
    let end = pts[pts.len() - 1];
    let d = unit(last_distinct(pts, end), end);
    pts.push([end[0] + d[0] * ext, end[1] + d[1] * ext]);
}

/// True when `p` lies on a tile border (multiples of `tile_m`, in map meters).
fn on_tile_border(p: [f64; 2], tile_m: f64) -> bool {
    let on = |v: f64| {
        let r = v.rem_euclid(tile_m);
        r < 0.01 || tile_m - r < 0.01
    };
    on(p[0]) || on(p[1])
}

/// Distance the last point of `pts` must move along its direction for the corners of its
/// band to clear the near edge of every other road it meets. Zero when it meets none.
fn extend_end(pts: &[[f64; 2]], half: f64, roads: &[Polyline], own: usize, tile_m: f64) -> f64 {
    let end = pts[pts.len() - 1];
    let prev = last_distinct(pts, end);
    if dist(prev, end) <= 1e-9 {
        return 0.0;
    }
    if on_tile_border(end, tile_m) {
        return BORDER_OVERLAP_M;
    }
    let d = unit(prev, end);
    let nrm = [-d[1], d[0]];
    let mut ext: f64 = 0.0;
    for (j, other) in roads.iter().enumerate() {
        if j == own || other.points.len() < 2 {
            continue;
        }
        let half_b = road_width_m(&other.highway) / 2.0;
        // Closest segment of the other road to the end.
        let mut best: Option<(f64, [f64; 2], [f64; 2])> = None;
        for w in other.points.windows(2) {
            let dd = dist_to_segment(end, w[0], w[1]);
            if best.is_none_or(|(b, _, _)| dd < b) {
                best = Some((dd, w[0], w[1]));
            }
        }
        let Some((dd, c0, c1)) = best else { continue };
        if dd > half_b + JUNCTION_TOL_M || dist(c0, c1) <= 1e-9 {
            continue;
        }
        let nb = unit_normal(c0, c1);
        // Rate at which moving along d changes the offset from the other road's axis.
        let u = d[0] * nb[0] + d[1] * nb[1];
        if u.abs() < 1e-3 {
            continue;
        }
        // Target: the near edge of the other road, on the side this road comes from.
        let target = -u.signum() * half_b;
        let mut need: f64 = 0.0;
        for sign in [-1.0, 1.0] {
            let corner = [end[0] + sign * half * nrm[0], end[1] + sign * half * nrm[1]];
            let g = (corner[0] - c0[0]) * nb[0] + (corner[1] - c0[1]) * nb[1];
            need = need.max((target - g) / u);
        }
        let cap = 2.0 * (half + half_b);
        ext = ext.max((need + 0.3).min(cap));
    }
    ext
}

/// The last point before `end` in `pts` that differs from it (or `end` if none).
fn last_distinct(pts: &[[f64; 2]], end: [f64; 2]) -> [f64; 2] {
    pts.iter()
        .rev()
        .copied()
        .find(|&p| dist(p, end) > 1e-9)
        .unwrap_or(end)
}
