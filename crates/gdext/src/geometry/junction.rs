//! Junction geometry: extends road ends so ribbons meet at crossings and overlap across tile
//! borders. Pure Rust (no Godot types). Imports `geometry::vec2` and `tileformat`/`tilebake` data.
use std::collections::BTreeMap;

use tileformat::{road_width_m, Polyline, Tile};

use super::road::Ribbon;
use super::vec2::{dist, dist_to_segment, unit, unit_normal};

/// How far from another road's band an end still counts as a junction, in meters.
const JUNCTION_TOL_M: f64 = 1.0;
/// How far a road end on a tile border runs past it, in meters, so the ribbons of the two
/// tiles overlap and no hairline of terrain shows through the seam.
pub(super) const BORDER_OVERLAP_M: f64 = 0.25;
/// A neighbor road leaving a seam node at an angle of at least about one degree to the
/// road ending there is a corner: the ends must then overlap by a full road half width.
const SEAM_STRAIGHT_COS: f64 = 0.999_85;

/// Points of every road of a tile, with each end that meets another road extended so that
/// its cut lies past the near edge of the crossed road. Ends on a tile border run a
/// little past it (overlap with the next tile); ends with no other road close by
/// (cul-de-sacs) are kept as they are.
/// Points of each road (extended ends included). Heights come from the terrain field, not here.
pub(super) fn extend_junction_ends(
    roads: &[Polyline],
    around: &[&Tile],
    tile_m: f64,
) -> Vec<Vec<[f64; 2]>> {
    roads
        .iter()
        .enumerate()
        .map(|(i, road)| {
            let half = road_width_m(&road.highway) / 2.0;
            let mut pts = road.points.clone();
            if pts.len() < 2 {
                return pts;
            }
            let head = extend_end(&pts, half, roads, around, i, tile_m);
            extend_tail(&mut pts, head);
            pts.reverse();
            let tail = extend_end(&pts, half, roads, around, i, tile_m);
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
fn extend_end(
    pts: &[[f64; 2]],
    half: f64,
    roads: &[Polyline],
    around: &[&Tile],
    own: usize,
    tile_m: f64,
) -> f64 {
    let end = pts[pts.len() - 1];
    let prev = last_distinct(pts, end);
    if dist(prev, end) <= 1e-9 {
        return 0.0;
    }
    if on_tile_border(end, tile_m) {
        // A corner exactly on the seam: the neighbor's piece turns there, and a flat cut
        // would leave the outer corner of the bend uncovered by either tile.
        let d_border = unit(prev, end);
        return match seam_corner_half(end, d_border, around) {
            Some(half_other) => half.max(half_other),
            None => BORDER_OVERLAP_M,
        };
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
            if best.map_or(true, |(b, _, _)| dd < b) {
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

/// Half width of a neighbor-tile road that bends at `end` (a node on the seam, reached along
/// `d`), `None` when no neighbor road leaves `end` along a different direction.
fn seam_corner_half(end: [f64; 2], d: [f64; 2], around: &[&Tile]) -> Option<f64> {
    let mut best: Option<f64> = None;
    for tile in around {
        for road in &tile.roads {
            for (k, &p) in road.points.iter().enumerate() {
                if dist(p, end) > 1e-6 {
                    continue;
                }
                let adjacent = [k.checked_sub(1), k.checked_add(1)];
                for q in adjacent
                    .into_iter()
                    .flatten()
                    .filter_map(|j| road.points.get(j))
                {
                    if dist(*q, end) <= 1e-9 {
                        continue;
                    }
                    let g = unit(end, *q);
                    if (g[0] * d[0] + g[1] * d[1]).abs() < SEAM_STRAIGHT_COS {
                        let h = road_width_m(&road.highway) / 2.0;
                        best = Some(best.map_or(h, |b| b.max(h)));
                    }
                }
            }
        }
    }
    best
}

/// Gives every ribbon meeting at a shared node one height at that node, so the junction reads as
/// a single surface and no ribbon steps over the other where they cross.
///
/// The highest ribbon's center height at the node is the floor: every lower ribbon is raised to
/// it, easing the extra lift back over the neighboring stations (`Ribbon::raise_at`). Repeated a
/// few times because a raise can reach the next node along a road. Only lifts go up, so no
/// ribbon sinks into the terrain. Nodes are matched with the bake's key (`field::NODE_KEY_M`), so
/// only places the profile reconciliation already treats as one node are joined. The map is
/// ordered, so the result is the same on every run (tile geometry stays reproducible).
pub(super) fn reconcile_lift(ribbons: &mut [(usize, Ribbon)]) {
    /// Two ribbon stations within half of this are the same node (`field::NODE_KEY_M`).
    const NODE_KEY_M: f64 = 0.1;
    /// Passes over the nodes: a raise can reach the next node along a road, so a few settle it.
    const PASSES: usize = 4;
    /// A larger gap between the two surfaces at a matched node is a grade separation (or bad
    /// data), not a junction: welding it would build a ramp into the sky, so it is left alone.
    const WELD_MAX_M: f32 = 0.5;
    let mut nodes: BTreeMap<(i64, i64), Vec<(usize, usize)>> = BTreeMap::new();
    for (slot, (_, ribbon)) in ribbons.iter().enumerate() {
        for &s in ribbon.node_stations() {
            let c = ribbon.centers()[s];
            let key = (
                (c[0] / NODE_KEY_M).round() as i64,
                (c[1] / NODE_KEY_M).round() as i64,
            );
            nodes.entry(key).or_default().push((slot, s));
        }
    }
    for _ in 0..PASSES {
        let mut raised = false;
        for entries in nodes.values() {
            if entries.len() < 2 {
                continue;
            }
            let height = |slot: usize, s: usize| {
                let r = &ribbons[slot].1;
                r.center_height(s) + r.extra_at(s)
            };
            let top = entries
                .iter()
                .map(|&(slot, s)| height(slot, s))
                .fold(f32::MIN, f32::max);
            let bottom = entries
                .iter()
                .map(|&(slot, s)| height(slot, s))
                .fold(f32::MAX, f32::min);
            if top - bottom > WELD_MAX_M {
                continue;
            }
            for &(slot, s) in entries {
                let r = &ribbons[slot].1;
                let need = top - (r.center_height(s) + r.extra_at(s));
                if need > 1e-6 {
                    let value = r.extra_at(s) + need;
                    ribbons[slot].1.raise_at(s, value);
                    raised = true;
                }
            }
        }
        if !raised {
            break;
        }
    }
}

/// The last point before `end` in `pts` that differs from it (or `end` if none).
fn last_distinct(pts: &[[f64; 2]], end: [f64; 2]) -> [f64; 2] {
    pts.iter()
        .rev()
        .copied()
        .find(|&p| dist(p, end) > 1e-9)
        .unwrap_or(end)
}
