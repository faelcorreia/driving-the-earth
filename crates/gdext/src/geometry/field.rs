//! Terrain surface field of a tile: the height that the terrain mesh (vertices on the fine grid)
//! and the road ribbons (at their own stations) both sample. Pure Rust (no Godot types).
//! Imports `geometry::vec2` and `tileformat` data (`units`, `Polyline`).
//!
//! The field is the 10 m grid (`ground`), except beside a road that has a profile. Over the
//! road's corridor (half the width on each side of its axis) it is the profile: `z` on the axis,
//! linear across to `zl` and `zr` on the edges. Over the next `ROAD_BLEND_M` it blends from the
//! edge height to the grid. Where corridors overlap, the lowest candidate wins, so a terrain
//! vertex never rises above a ribbon in a corridor. Roads with no profile do not change the grid.
use tileformat::units::{ROAD_BLEND_M, TERRAIN_FINE_CELL_M, TILE_M};
use tileformat::Tile;

use super::profile_smooth::smooth_profile;
use super::terrain::sample_height;
use tileformat::{road_width_m, Polyline};

use super::vec2::unit_normal;

/// Side of a bucket of the segment index, in meters.
const BUCKET_M: f64 = 10.0;

/// Half-width of the flat zone of a road: the ribbon edge plus one blend cell, rounded up to
/// whole fine cells. Every grid vertex that a ribbon triangle touches then takes the profile
/// height, so the ribbon edge never sits on a triangle lowered by the grid beyond it.
pub(super) fn flat_zone_m(half: f64) -> f64 {
    ((half + ROAD_BLEND_M) / TERRAIN_FINE_CELL_M).ceil() * TERRAIN_FINE_CELL_M
}

/// One segment of a road with a profile, with the profile at its two ends.
struct Seg {
    a: [f64; 2],
    dir: [f64; 2],
    len: f64,
    /// Unit normal pointing to the left edge (the side the ribbon calls `left`).
    nrm: [f64; 2],
    half: f64,
    /// Half-width of the flat zone (`flat_zone_m`); the blend starts beyond it.
    flat: f64,
    z: [f32; 2],
    zl: [f32; 2],
    zr: [f32; 2],
}

impl Seg {
    /// The field candidate of this segment at `(x, y)`, given the grid height `g` there.
    /// `None` outside the corridor and its blend band.
    fn value(&self, x: f64, y: f64, g: f32) -> Option<f32> {
        let p = [x - self.a[0], y - self.a[1]];
        let t = ((p[0] * self.dir[0] + p[1] * self.dir[1]) / self.len).clamp(0.0, 1.0);
        let (qx, qy) = (p[0] - self.dir[0] * self.len * t, p[1] - self.dir[1] * self.len * t);
        let dist = qx.hypot(qy);
        if dist > self.flat + ROAD_BLEND_M {
            return None;
        }
        let s = (qx * self.nrm[0] + qy * self.nrm[1]).clamp(-self.half, self.half);
        let t = t as f32;
        let at = |v: [f32; 2]| v[0] + (v[1] - v[0]) * t;
        let (zc, zl, zr) = (at(self.z), at(self.zl), at(self.zr));
        let k = (s / self.half) as f32;
        let edge = if k >= 0.0 { zc + (zl - zc) * k } else { zc + (zr - zc) * -k };
        if dist <= self.flat {
            Some(edge)
        } else {
            let u = ((dist - self.flat) / ROAD_BLEND_M).clamp(0.0, 1.0) as f32;
            Some(edge + (g - edge) * u)
        }
    }
}

/// Segments of the roads with a profile, indexed in buckets over the tile and its neighbors.
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
        for road in roads {
            field.add_road(road);
        }
        field
    }

    fn add_road(&mut self, road: &Polyline) {
        let n = road.points.len();
        if n < 2 || road.z.len() != n {
            return;
        }
        // The profile the field samples is the runtime-smoothed one (same points, same stations).
        let (z_all, zl_all, zr_all) = smooth_profile(&road.points, &road.z, &road.zl, &road.zr, &road.highway);
        let has_edges = zl_all.len() == n && zr_all.len() == n;
        let half = road_width_m(&road.highway) / 2.0;
        let flat = flat_zone_m(half);
        self.reach = self.reach.max(flat + ROAD_BLEND_M);
        for k in 0..n - 1 {
            let (a, b) = (road.points[k], road.points[k + 1]);
            let len = (b[0] - a[0]).hypot(b[1] - a[1]);
            if len <= 1e-9 {
                continue;
            }
            let (z, zl, zr) = if has_edges {
                ([z_all[k], z_all[k + 1]], [zl_all[k], zl_all[k + 1]], [zr_all[k], zr_all[k + 1]])
            } else {
                let z = [z_all[k], z_all[k + 1]];
                (z, z, z)
            };
            let seg = Seg {
                a,
                dir: [(b[0] - a[0]) / len, (b[1] - a[1]) / len],
                len,
                nrm: unit_normal(a, b),
                half,
                flat,
                z,
                zl,
                zr,
            };
            let reach = flat + ROAD_BLEND_M;
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
        let c = ((x - self.origin[0]) / BUCKET_M).floor().clamp(0.0, (self.cols - 1) as f64);
        let r = ((y - self.origin[1]) / BUCKET_M).floor().clamp(0.0, (self.rows - 1) as f64);
        (c as usize, r as usize)
    }

    /// The surface height at map point `(x, y)`: the lowest road candidate, or the grid `ground`.
    pub(super) fn height(&self, ground: &dyn Fn(f64, f64) -> f32, x: f64, y: f64) -> f32 {
        let g = ground(x, y);
        let (c0, r0) = self.cell(x - self.reach, y - self.reach);
        let (c1, r1) = self.cell(x + self.reach, y + self.reach);
        let mut best: Option<f32> = None;
        for r in r0..=r1 {
            for c in c0..=c1 {
                for &k in &self.buckets[r * self.cols + c] {
                    if let Some(v) = self.segs[k as usize].value(x, y, g) {
                        best = Some(best.map_or(v, |b| b.min(v)));
                    }
                }
            }
        }
        best.unwrap_or(g)
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
    pub(super) fn new(field: Field, tile: &Tile, tile_m: f64, ground: &dyn Fn(f64, f64) -> f32, origin: [f64; 2], n: usize, cell: f64) -> Surface {
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
        let mut best = vec![f32::INFINITY; n * n];
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
                    if let Some(v) = seg.value(x, y, g) {
                        let b = &mut best[i * n + j];
                        if v < *b {
                            *b = v;
                        }
                    }
                }
            }
        }
        for (h, b) in grid.iter_mut().zip(&best) {
            if b.is_finite() {
                *h = *b;
            }
        }
        Surface { heights: grid, origin, n, cell, field }
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
