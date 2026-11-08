//! Terrain surface field of a tile: the height that the terrain mesh (vertices on the fine grid)
//! and the road ribbons (at their own stations) both sample. Pure Rust (no Godot types).
//! Imports `geometry::vec2` and `tileformat` data (`units`, `Polyline`).
//!
//! The field is the 10 m grid (`ground`), except beside a road that has a profile. Over the
//! road's corridor (half the width on each side of its axis) it is the profile: `z` on the axis,
//! linear across to `zl` and `zr` on the edges. Over the next `ROAD_BLEND_M` it blends from the
//! edge height to the grid. Where corridors overlap, a flat-zone candidate wins over a blend-band
//! candidate of another road; between two flat-zone candidates the one nearer its axis wins, and
//! between two blend-band candidates the lowest wins. Roads with no profile do not change the grid.
//!
//! Where roads share a node (same point, `NODE_KEY_M`), the profiles are reconciled first: the
//! node takes one height, the mean of the roads through it (of all of them when every road ends
//! there), and each road eases that offset back to zero over `NODE_EASE_M` along its own axis.
//! So two roads meeting at a node leave it at the same height.
use std::collections::HashMap;

use tileformat::units::{ROAD_BLEND_M, TERRAIN_FINE_CELL_M, TILE_M};
use tileformat::Tile;

use super::profile_smooth::smooth_profile;
use super::terrain::sample_height;
use tileformat::{road_width_m, Polyline};

use super::vec2::unit_normal;

/// Side of a bucket of the segment index, in meters.
const BUCKET_M: f64 = 10.0;

/// Length beyond an open end of a road (no neighbor segment there) over which the profile keeps
/// its slope, in meters. Beyond it the cap is flat again.
const OPEN_END_EXTEND_M: f64 = 4.0;

/// Grid of the shared-node keys: two road nodes within half of this are the same node.
const NODE_KEY_M: f64 = 0.1;

/// Distance along a road over which the offset of a shared node decays linearly to zero.
const NODE_EASE_M: f64 = 12.0;

/// One surface candidate at a point: whether it is in its road's flat zone, the distance to that
/// road's axis, and the height it gives.
#[derive(Clone, Copy)]
struct Cand {
    flat: bool,
    dist: f64,
    h: f32,
}

/// Whether candidate `a` beats `b` (candidates of different roads). A flat-zone candidate beats a
/// blend-band one; between two flat-zone candidates the nearer axis wins; between two blend-band
/// candidates the lower height wins.
fn beats(a: Cand, b: Cand) -> bool {
    if a.flat != b.flat {
        a.flat
    } else if a.flat {
        a.dist < b.dist
    } else {
        a.h < b.h
    }
}

/// Half-width of the flat zone of a road: the ribbon edge plus one blend cell, rounded up to
/// whole fine cells. Every grid vertex that a ribbon triangle touches then takes the profile
/// height, so the ribbon edge never sits on a triangle lowered by the grid beyond it.
pub(super) fn flat_zone_m(half: f64) -> f64 {
    ((half + ROAD_BLEND_M) / TERRAIN_FINE_CELL_M).ceil() * TERRAIN_FINE_CELL_M
}

/// One segment of a road with a profile, with the profile at its two ends.
#[derive(Clone)]
struct Seg {
    a: [f64; 2],
    dir: [f64; 2],
    len: f64,
    /// Unit normal pointing to the left edge (the side the ribbon calls `left`).
    nrm: [f64; 2],
    /// Index of the road of this segment. The segments of one road are contiguous in `Field::segs`.
    road: u32,
    /// The segment start / end is a node shared with a neighbor segment of the same road (not an
    /// end of the road): beyond it, the lateral coordinate is the radial distance, so both
    /// segments agree on the bisector of the bend.
    joint: [bool; 2],
    half: f64,
    /// Half-width of the flat zone (`flat_zone_m`); the blend starts beyond it.
    flat: f64,
    z: [f32; 2],
    zl: [f32; 2],
    zr: [f32; 2],
}

impl Seg {
    /// The field candidate of this segment at `(x, y)`, given the grid height `g` there, the
    /// distance from `(x, y)` to this segment's axis, and whether it lies in the flat zone. `None`
    /// outside the corridor and its blend band.
    fn value(&self, x: f64, y: f64, g: f32) -> Option<(f32, f64, bool)> {
        let p = [x - self.a[0], y - self.a[1]];
        let t_raw = (p[0] * self.dir[0] + p[1] * self.dir[1]) / self.len;
        // Along the axis the profile stops at a joint (the neighbor segment takes over there) and
        // runs on its own slope up to `OPEN_END_EXTEND_M` past an open end.
        let lo = if self.joint[0] {
            0.0
        } else {
            -OPEN_END_EXTEND_M / self.len
        };
        let hi = if self.joint[1] {
            1.0
        } else {
            1.0 + OPEN_END_EXTEND_M / self.len
        };
        let t_z = t_raw.clamp(lo, hi);
        let t = t_raw.clamp(0.0, 1.0);
        let (qx, qy) = (
            p[0] - self.dir[0] * self.len * t,
            p[1] - self.dir[1] * self.len * t,
        );
        // Cheap reject before the square root: beyond the reach the result is `None` anyway.
        let reach = self.flat + ROAD_BLEND_M;
        if qx * qx + qy * qy > reach * reach * (1.0 + 1e-9) {
            return None;
        }
        let dist = qx.hypot(qy);
        if dist > reach {
            return None;
        }
        let side = qx * self.nrm[0] + qy * self.nrm[1];
        // Beyond a joint node the nearest point is the node itself and `side` depends on this
        // segment's normal, so the two segments would disagree on the bisector (a jump of the
        // cross slope). The radial distance with the side of this segment is the same for both.
        let beyond_joint = (t_raw < 0.0 && self.joint[0]) || (t_raw > 1.0 && self.joint[1]);
        let lateral = if beyond_joint {
            dist.copysign(side)
        } else {
            side
        };
        let s = lateral.clamp(-self.half, self.half);
        let t_z = t_z as f32;
        let at = |v: [f32; 2]| v[0] + (v[1] - v[0]) * t_z;
        let (zc, zl, zr) = (at(self.z), at(self.zl), at(self.zr));
        let k = (s / self.half) as f32;
        let edge = if k >= 0.0 {
            zc + (zl - zc) * k
        } else {
            zc + (zr - zc) * -k
        };
        if dist <= self.flat {
            Some((edge, dist, true))
        } else {
            let u = ((dist - self.flat) / ROAD_BLEND_M).clamp(0.0, 1.0) as f32;
            Some((edge + (g - edge) * u, dist, false))
        }
    }
}

/// A road with a profile, smoothed and ready to become segments. `z`, `zl` and `zr` are the
/// smoothed heights at each point (`zl` and `zr` equal `z` when the road has no edge heights).
struct Profile<'a> {
    road: u32,
    points: &'a [[f64; 2]],
    /// Arc length from the first point to each point, in meters.
    arc: Vec<f64>,
    z: Vec<f32>,
    zl: Vec<f32>,
    zr: Vec<f32>,
    half: f64,
    flat: f64,
}

impl<'a> Profile<'a> {
    /// The profile of `road` (index `road_id` among the roads of the block), or `None` when the
    /// road has no profile.
    fn of(road: &'a Polyline, road_id: u32) -> Option<Profile<'a>> {
        let n = road.points.len();
        if n < 2 || road.z.len() != n {
            return None;
        }
        // The profile the field samples is the runtime-smoothed one (same points, same stations).
        let (z, zl, zr) = smooth_profile(&road.points, &road.z, &road.zl, &road.zr, &road.highway);
        let (zl, zr) = if zl.len() == n && zr.len() == n {
            (zl, zr)
        } else {
            (z.clone(), z.clone())
        };
        let mut arc = Vec::with_capacity(n);
        let mut s = 0.0;
        for (k, p) in road.points.iter().enumerate() {
            if k > 0 {
                let q = road.points[k - 1];
                s += (p[0] - q[0]).hypot(p[1] - q[1]);
            }
            arc.push(s);
        }
        let half = road_width_m(&road.highway) / 2.0;
        Some(Profile {
            road: road_id,
            points: &road.points,
            arc,
            z,
            zl,
            zr,
            half,
            flat: flat_zone_m(half),
        })
    }
}

/// Gives every shared node of the profiles one height and eases the offset back along each road
/// (see the module docs). A node is shared when two roads have points within `NODE_KEY_M` of it;
/// a road's own node, or one no other road shares, has no offset.
fn reconcile_nodes(profiles: &mut [Profile]) {
    // Node key -> (profile index, point index, passes through: not an end of its road).
    let mut nodes: HashMap<(i64, i64), Vec<(usize, usize, bool)>> = HashMap::new();
    for (p_idx, p) in profiles.iter().enumerate() {
        let n = p.points.len();
        for (k, q) in p.points.iter().enumerate() {
            let key = (
                (q[0] / NODE_KEY_M).round() as i64,
                (q[1] / NODE_KEY_M).round() as i64,
            );
            nodes
                .entry(key)
                .or_default()
                .push((p_idx, k, k > 0 && k + 1 < n));
        }
    }
    // Per profile and point: the offset its height needs to reach the node target.
    let mut offsets: Vec<Vec<f32>> = profiles.iter().map(|p| vec![0.0; p.points.len()]).collect();
    for entries in nodes.values() {
        if entries.len() < 2 {
            continue;
        }
        let through: Vec<&(usize, usize, bool)> = entries.iter().filter(|e| e.2).collect();
        let group: Vec<&(usize, usize, bool)> = if through.is_empty() {
            entries.iter().collect()
        } else {
            through
        };
        let sum: f64 = group.iter().map(|e| profiles[e.0].z[e.1] as f64).sum();
        let target = (sum / group.len() as f64) as f32;
        for &(p_idx, k, _) in entries {
            offsets[p_idx][k] = target - profiles[p_idx].z[k];
        }
    }
    for (p, offs) in profiles.iter_mut().zip(&offsets) {
        if offs.iter().all(|&o| o == 0.0) {
            continue;
        }
        // Each point takes the sum of the offsets of the nodes of its road within `NODE_EASE_M`
        // of it, each decayed linearly with the arc distance to it.
        let n = p.points.len();
        let mut best_o = vec![0.0f32; n];
        for (k, &o) in offs.iter().enumerate() {
            let s = p.arc[k];
            let lo = p.arc.partition_point(|&a| a <= s - NODE_EASE_M);
            for j in lo..n {
                if p.arc[j] >= s + NODE_EASE_M {
                    break;
                }
                let d = (p.arc[j] - s).abs();
                best_o[j] += o * (1.0 - d / NODE_EASE_M) as f32;
            }
        }
        for j in 0..n {
            let o = best_o[j];
            p.z[j] += o;
            p.zl[j] += o;
            p.zr[j] += o;
        }
    }
}

/// Segments of the roads with a profile, indexed in buckets over the tile and its neighbors.
#[derive(Clone)]
pub(super) struct Field {
    segs: Vec<Seg>,
    origin: [f64; 2],
    cols: usize,
    rows: usize,
    buckets: Vec<Vec<u32>>,
    /// Largest distance from an axis at which a segment still has a candidate.
    reach: f64,
}

impl Field {
    /// True when no road of the tile and its neighbors has a profile: the field changes no height.
    pub(super) fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }

    /// Index of the roads of a 3 x 3 block of tiles whose corner is `origin` (map meters).
    pub(super) fn new<'a>(roads: impl Iterator<Item = &'a Polyline>, origin: [f64; 2]) -> Field {
        let span = 3.0 * TILE_M;
        let cols = (span / BUCKET_M).ceil() as usize;
        let mut field = Field {
            segs: Vec::new(),
            origin,
            cols,
            rows: cols,
            buckets: vec![Vec::new(); cols * cols],
            reach: 0.0,
        };
        let mut profiles: Vec<Profile> = roads
            .enumerate()
            .filter_map(|(road_id, road)| Profile::of(road, road_id as u32))
            .collect();
        reconcile_nodes(&mut profiles);
        for p in &profiles {
            field.add_profile(p);
        }
        field
    }

    fn add_profile(&mut self, p: &Profile) {
        let n = p.points.len();
        self.reach = self.reach.max(p.flat + ROAD_BLEND_M);
        for k in 0..n - 1 {
            let (a, b) = (p.points[k], p.points[k + 1]);
            let len = (b[0] - a[0]).hypot(b[1] - a[1]);
            if len <= 1e-9 {
                continue;
            }
            let seg = Seg {
                a,
                dir: [(b[0] - a[0]) / len, (b[1] - a[1]) / len],
                len,
                nrm: unit_normal(a, b),
                road: p.road,
                joint: [k > 0, k + 2 < n],
                half: p.half,
                flat: p.flat,
                z: [p.z[k], p.z[k + 1]],
                zl: [p.zl[k], p.zl[k + 1]],
                zr: [p.zr[k], p.zr[k + 1]],
            };
            let reach = p.flat + ROAD_BLEND_M;
            let (c0, r0) = self.cell(a[0].min(b[0]) - reach, a[1].min(b[1]) - reach);
            let (c1, r1) = self.cell(a[0].max(b[0]) + reach, a[1].max(b[1]) + reach);
            let idx = self.segs.len() as u32;
            for r in r0..=r1 {
                for c in c0..=c1 {
                    self.buckets[r * self.cols + c].push(idx);
                }
            }
            self.segs.push(seg);
        }
    }

    /// Bucket (column, row) of a map point, clamped to the block.
    fn cell(&self, x: f64, y: f64) -> (usize, usize) {
        let c = ((x - self.origin[0]) / BUCKET_M)
            .floor()
            .clamp(0.0, (self.cols - 1) as f64);
        let r = ((y - self.origin[1]) / BUCKET_M)
            .floor()
            .clamp(0.0, (self.rows - 1) as f64);
        (c as usize, r as usize)
    }

    /// The surface height at map point `(x, y)`: the winning road candidate, or the grid `ground`.
    /// Within one road only the candidate of its segment nearest to `(x, y)` counts, so the end
    /// cap of a segment never stands in for a point beside the next segment of the same road.
    /// Between roads a flat-zone candidate beats a blend-band one, then the lowest wins (`beats`).
    pub(super) fn height(&self, ground: &dyn Fn(f64, f64) -> f32, x: f64, y: f64) -> f32 {
        let g = ground(x, y);
        let (c0, r0) = self.cell(x - self.reach, y - self.reach);
        let (c1, r1) = self.cell(x + self.reach, y + self.reach);
        // Per road: (road, distance to its nearest axis, candidate there, flat zone). A point sees
        // a few roads, so the first `INLINE_ROADS` live on the stack and no heap block is allocated.
        const INLINE_ROADS: usize = 8;
        let mut inline = [(u32::MAX, f64::INFINITY, 0.0f32, false); INLINE_ROADS];
        let mut inline_len = 0usize;
        let mut spill: Vec<(u32, f64, f32, bool)> = Vec::new();
        for r in r0..=r1 {
            for c in c0..=c1 {
                for &k in &self.buckets[r * self.cols + c] {
                    let seg = &self.segs[k as usize];
                    if let Some((v, d, flat)) = seg.value(x, y, g) {
                        let slot = inline[..inline_len]
                            .iter_mut()
                            .chain(spill.iter_mut())
                            .find(|e| e.0 == seg.road);
                        match slot {
                            Some(e) => {
                                if d < e.1 {
                                    e.1 = d;
                                    e.2 = v;
                                    e.3 = flat;
                                }
                            }
                            None if inline_len < INLINE_ROADS => {
                                inline[inline_len] = (seg.road, d, v, flat);
                                inline_len += 1;
                            }
                            None => spill.push((seg.road, d, v, flat)),
                        }
                    }
                }
            }
        }
        inline[..inline_len]
            .iter()
            .chain(spill.iter())
            .map(|e| Cand {
                flat: e.3,
                dist: e.1,
                h: e.2,
            })
            .reduce(|a, b| if beats(b, a) { b } else { a })
            .map_or(g, |best| best.h)
    }
}

/// The surface of one tile: the fine grid (`heights`, row-major, `n` per side, `cell` apart,
/// origin at the tile corner) rasterized from the field, and the field itself for points off the grid.
pub(super) struct Surface {
    pub(super) heights: Vec<f32>,
    origin: [f64; 2],
    n: usize,
    cell: f64,
    field: Field,
}

impl Surface {
    /// Rasterizes the road candidates on the fine grid, segment by segment: each segment only
    /// visits the vertices in its reach. Vertices with no candidate keep the grid height.
    pub(super) fn new(
        field: Field,
        tile: &Tile,
        tile_m: f64,
        ground: &dyn Fn(f64, f64) -> f32,
        origin: [f64; 2],
        n: usize,
        cell: f64,
    ) -> Surface {
        let mut grid = vec![0.0f32; n * n];
        for i in 0..n {
            for j in 0..n {
                let (x, y) = (origin[0] + j as f64 * cell, origin[1] + i as f64 * cell);
                // Off the last row and column the point lies in this tile: its own grid is the ground.
                grid[i * n + j] = if i + 1 < n && j + 1 < n {
                    sample_height(tile, tile_m, x, y)
                } else {
                    ground(x, y)
                };
            }
        }
        // Per vertex: the winning candidate of the roads finished so far (`best`), and the nearest
        // segment of the road being visited (`cur`). Roads are contiguous in `field.segs`.
        let none = Cand {
            flat: false,
            dist: f64::INFINITY,
            h: f32::INFINITY,
        };
        let mut best = vec![none; n * n];
        let mut cur_road = vec![u32::MAX; n * n];
        let mut cur = vec![none; n * n];
        for seg in &field.segs {
            let reach = seg.flat + ROAD_BLEND_M;
            let x0 = seg.a[0].min(seg.a[0] + seg.dir[0] * seg.len) - reach;
            let x1 = seg.a[0].max(seg.a[0] + seg.dir[0] * seg.len) + reach;
            let y0 = seg.a[1].min(seg.a[1] + seg.dir[1] * seg.len) - reach;
            let y1 = seg.a[1].max(seg.a[1] + seg.dir[1] * seg.len) + reach;
            let j0 = ((x0 - origin[0]) / cell).floor().max(0.0) as usize;
            let j1 = (((x1 - origin[0]) / cell).ceil().max(0.0) as usize).min(n - 1);
            let i0 = ((y0 - origin[1]) / cell).floor().max(0.0) as usize;
            let i1 = (((y1 - origin[1]) / cell).ceil().max(0.0) as usize).min(n - 1);
            for i in i0..=i1 {
                for j in j0..=j1 {
                    let (x, y) = (origin[0] + j as f64 * cell, origin[1] + i as f64 * cell);
                    let g = grid[i * n + j];
                    if let Some((v, d, flat)) = seg.value(x, y, g) {
                        let idx = i * n + j;
                        let cand = Cand {
                            flat,
                            dist: d,
                            h: v,
                        };
                        if cur_road[idx] != seg.road {
                            // The previous road at this vertex is finished: it competes by its candidate.
                            if beats(cur[idx], best[idx]) {
                                best[idx] = cur[idx];
                            }
                            cur_road[idx] = seg.road;
                            cur[idx] = cand;
                        } else if d < cur[idx].dist {
                            cur[idx] = cand;
                        }
                    }
                }
            }
        }
        for idx in 0..n * n {
            if beats(cur[idx], best[idx]) {
                best[idx] = cur[idx];
            }
        }
        for (h, b) in grid.iter_mut().zip(&best) {
            if b.h.is_finite() {
                *h = b.h;
            }
        }
        Surface {
            heights: grid,
            origin,
            n,
            cell,
            field,
        }
    }

    /// The surface on the collision grid: `n` vertices per side over `cell` meters, from the tile corner
    /// `origin`. The interior vertices are the drawn grid: their grid height is the recorded one, so
    /// the candidates they took are the ones the collision needs. The last row and column have the
    /// ground of the mesh (`Surface::new`), not the recorded one, so they take the field with the
    /// recorded ground instead, which is what a candidate there would have used.
    pub(super) fn collision_grid(&self, recorded: &dyn Fn(f64, f64) -> f32) -> Vec<f32> {
        let n = self.n;
        let mut out = self.heights.clone();
        for i in 0..n {
            for j in 0..n {
                if i + 1 == n || j + 1 == n {
                    let (x, y) = (
                        self.origin[0] + j as f64 * self.cell,
                        self.origin[1] + i as f64 * self.cell,
                    );
                    out[i * n + j] = self.field.height(recorded, x, y);
                }
            }
        }
        out
    }

    /// Height at `(x, y)`: the triangle sample of the grid inside the grid (the same triangles the
    /// mesh draws, so ribbons lie on the mesh), the field outside it.
    pub(super) fn height(&self, ground: &dyn Fn(f64, f64) -> f32, x: f64, y: f64) -> f32 {
        let n = self.n;
        let u = (x - self.origin[0]) / self.cell;
        let v = (y - self.origin[1]) / self.cell;
        if u < 0.0 || v < 0.0 || u > (n - 1) as f64 || v > (n - 1) as f64 {
            return self.field.height(ground, x, y);
        }
        let j0 = (u.floor() as usize).min(n - 2);
        let i0 = (v.floor() as usize).min(n - 2);
        let (fu, fv) = ((u - j0 as f64) as f32, (v - i0 as f64) as f32);
        let h = |i: usize, j: usize| self.heights[i * n + j];
        let (a, b, c, d) = (h(i0, j0), h(i0, j0 + 1), h(i0 + 1, j0), h(i0 + 1, j0 + 1));
        // Same split as the mesh: (a, b, c) for u + v <= 1, else (d, b, c).
        if fu + fv <= 1.0 {
            a + (b - a) * fu + (c - a) * fv
        } else {
            d + (c - d) * (1.0 - fu) + (b - d) * (1.0 - fv)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn road(highway: &str, points: Vec<[f64; 2]>, z: f32) -> Polyline {
        let n = points.len();
        Polyline {
            oneway: None,
            lanes: None,
            maxspeed: None,
            roundabout: false,
            highway: highway.into(),
            name: String::new(),
            points,
            z: vec![z; n],
            zl: vec![z; n],
            zr: vec![z; n],
            lit: None,
        }
    }

    /// Road A (z 10) runs along x. Road B (z 2) crosses it at x = 50, and A's axis lies in B's
    /// blend band at mid-band, so B's candidate there is lower than A's. A's flat zone wins.
    #[test]
    fn flat_zone_of_a_road_beats_blend_band_of_another() {
        let a = road("residential", vec![[0.0, 0.0], [100.0, 0.0]], 10.0);
        let b = road("residential", vec![[50.0, -50.0], [50.0, 50.0]], 2.0);
        let flat = flat_zone_m(road_width_m("residential") / 2.0);
        let x = 50.0 + flat + ROAD_BLEND_M / 2.0;
        let field = Field::new([a, b].iter(), [-100.0, -100.0]);
        assert_eq!(field.height(&|_, _| 0.0, x, 0.0), 10.0);
    }

    /// A T junction at (0, 0): the main road passes through the node, the stem ends there. Their
    /// smoothed profiles differ at the node; after reconciliation both leave it at the same `z`.
    #[test]
    fn t_junction_node_has_one_height() {
        let main = Polyline {
            oneway: None,
            lanes: None,
            maxspeed: None,
            roundabout: false,
            highway: "residential".into(),
            name: String::new(),
            points: vec![[-50.0, 0.0], [0.0, 0.0], [50.0, 0.0]],
            z: vec![4.0, 6.0, 9.0],
            zl: Vec::new(),
            zr: Vec::new(),
            lit: None,
        };
        let stem = Polyline {
            oneway: None,
            lanes: None,
            maxspeed: None,
            roundabout: false,
            highway: "residential".into(),
            name: String::new(),
            points: vec![[0.0, 0.0], [0.0, 20.0], [0.0, 40.0]],
            z: vec![1.0, 2.0, 3.0],
            zl: Vec::new(),
            zr: Vec::new(),
            lit: None,
        };
        let roads = [main, stem];
        let mut profiles: Vec<Profile> = roads
            .iter()
            .enumerate()
            .filter_map(|(i, r)| Profile::of(r, i as u32))
            .collect();
        assert_ne!(
            profiles[0].z[1], profiles[1].z[0],
            "the profiles differ before reconciling"
        );
        reconcile_nodes(&mut profiles);
        assert!(
            (profiles[0].z[1] - profiles[1].z[0]).abs() < 1e-6,
            "node heights {} and {}",
            profiles[0].z[1],
            profiles[1].z[0]
        );
    }
}
