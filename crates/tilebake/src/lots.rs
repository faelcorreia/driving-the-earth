//! Generated lots (B0): fills the free urban lots of one tile with generated buildings and
//! marks the ground cover (`Tile::ground`). It works on a raster of `MODULE_M` cells over a
//! window of 3 x 3 tiles (the tile and its neighbours), so a lot is decided from the same
//! geometry in every tile that sees it. Lots are emitted only by the tile that holds their
//! center, so a neighbour never repeats them.
//!
//! Determinism: every random value comes from `hash` of stable keys (road index in the global
//! list, side, lot index along the road, block id). Blocks are used only when they fit inside
//! the window; a block that touches the window border is skipped (huge blocks get no lots).
use tileformat::units::lots::{
    ACCESS_SERVICE_MIN_M, EMPTY_LOT_PERMILLE, FOOTPRINT_MARGIN_M, FRONT_SETBACK_MAX_M, LOT_D_MAX_M,
    LOT_D_MIN_M, LOT_W_MAX_M, LOT_W_MIN_M, SIDE_SETBACK_MAX_M, TALL_MAIN_PERMILLE, TALL_PERMILLE,
};
use tileformat::units::{GROUND_CELL_M, GROUND_N, MODULE_M, SIDEWALK_M, TILE_M};
use tileformat::{
    road_width_m, Polygon, Polyline, GROUND_CONCRETE, GROUND_GRASS, GROUND_PARK, GROUND_SIDEWALK,
    GROUND_WATER,
};

/// Cells per side of the 3 x 3 tile window, and of one tile.
const TILE_CELLS: usize = (TILE_M / MODULE_M) as usize;
const WIN_N: usize = 3 * TILE_CELLS;
const CELLS_PER_GROUND: usize = (GROUND_CELL_M / MODULE_M) as usize;

const F_ROAD: u8 = 1;
const F_SIDEWALK: u8 = 2;
const F_FOOT: u8 = 4;
const F_BLOCK: u8 = 8;
const F_PARK: u8 = 16;
const F_WATER: u8 = 32;
const F_LOT: u8 = 64;
/// Cells that a lot may never use.
const BLOCKING: u8 = F_ROAD | F_SIDEWALK | F_BLOCK | F_PARK | F_WATER;

/// Kind of a green or water area of the OSM (not a building, not a road).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AreaKind {
    Park,
    Water,
}

/// Input of one tile: the global road list (index kept as the stable key), the real building
/// rings and the park/water rings, all in the local plane of the bake.
pub struct Window<'a> {
    pub tile: (i32, i32),
    pub roads: Vec<(usize, &'a Polyline)>,
    pub footprints: Vec<&'a [[f64; 2]]>,
    pub areas: Vec<(AreaKind, &'a [[f64; 2]])>,
    /// Waterway centerlines with their half width in meters (`river` 8 m, `stream` 2 m, ...): water cells.
    pub water: Vec<(f64, &'a [[f64; 2]])>,
}

/// Output of one tile: the generated buildings whose center is in the tile, and the ground
/// cover of the tile, row-major `GROUND_N` x `GROUND_N`.
#[derive(Debug, PartialEq)]
pub struct Output {
    pub buildings: Vec<Polygon>,
    pub ground: Vec<u8>,
}

/// Raster of the window: flags per cell, block label per cell (0 when not usable).
struct Raster {
    flags: Vec<u8>,
    label: Vec<u32>,
    origin: [f64; 2],
}

/// Accepted lot, kept until the building is generated.
struct Lot {
    block: u64,
    ri: usize,
    side: usize,
    k: u64,
    origin: [f64; 2],
    u: [f64; 2],
    v: [f64; 2],
    w: f64,
    d: f64,
    main: bool,
}

fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

pub fn hash(parts: &[u64]) -> u64 {
    parts.iter().fold(0x1234_5678_u64, |h, &p| mix(h ^ p))
}

struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = mix(self.0);
        self.0
    }
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn snap(x: f64) -> f64 {
    (x / MODULE_M).round() * MODULE_M
}

fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn scale(a: [f64; 2], s: f64) -> [f64; 2] {
    [a[0] * s, a[1] * s]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (qx, qy) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    (qx * qx + qy * qy).sqrt()
}

fn ring_dist(p: [f64; 2], ring: &[[f64; 2]]) -> f64 {
    (0..ring.len())
        .map(|k| seg_dist(p, ring[k], ring[(k + 1) % ring.len()]))
        .fold(f64::MAX, f64::min)
}

/// Crossing-number test.
fn inside_ring(p: [f64; 2], ring: &[[f64; 2]]) -> bool {
    let mut inside = false;
    for k in 0..ring.len() {
        let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1])
        {
            inside = !inside;
        }
    }
    inside
}

fn seg_len(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
}

fn polyline_len(pts: &[[f64; 2]]) -> f64 {
    pts.windows(2).map(|w| seg_len(w[0], w[1])).sum()
}

/// Roads that block blocks (urban classes; service only when long).
fn is_urban(highway: &str, len: f64) -> bool {
    match highway {
        "residential" | "unclassified" | "living_street" | "tertiary" | "tertiary_link"
        | "secondary" | "secondary_link" | "primary" | "primary_link" => true,
        "service" => len >= ACCESS_SERVICE_MIN_M,
        _ => false,
    }
}

impl Raster {
    fn idx(i: usize, j: usize) -> usize {
        j * WIN_N + i
    }

    fn center(&self, i: usize, j: usize) -> [f64; 2] {
        [
            self.origin[0] + (i as f64 + 0.5) * MODULE_M,
            self.origin[1] + (j as f64 + 0.5) * MODULE_M,
        ]
    }

    fn cell_of(&self, p: [f64; 2]) -> Option<(usize, usize)> {
        let i = ((p[0] - self.origin[0]) / MODULE_M).floor();
        let j = ((p[1] - self.origin[1]) / MODULE_M).floor();
        if i < 0.0 || j < 0.0 || i >= WIN_N as f64 || j >= WIN_N as f64 {
            None
        } else {
            Some((i as usize, j as usize))
        }
    }

    /// Cell ranges `(i0, i1, j0, j1)` covering the box, clamped to the window.
    fn span(&self, lo: [f64; 2], hi: [f64; 2]) -> (usize, usize, usize, usize) {
        let c = |v: f64| v.clamp(0.0, (WIN_N - 1) as f64) as usize;
        let i0 = c(((lo[0] - self.origin[0]) / MODULE_M).floor());
        let i1 = c(((hi[0] - self.origin[0]) / MODULE_M).floor());
        let j0 = c(((lo[1] - self.origin[1]) / MODULE_M).floor());
        let j1 = c(((hi[1] - self.origin[1]) / MODULE_M).floor());
        (i0, i1, j0, j1)
    }
}

/// Road band: cells within `hw` of the centerline are road, within `reach` are road band.
fn paint_band(r: &mut Raster, pts: &[[f64; 2]], hw: f64, reach: f64) {
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let lo = [a[0].min(b[0]) - reach, a[1].min(b[1]) - reach];
        let hi = [a[0].max(b[0]) + reach, a[1].max(b[1]) + reach];
        let (i0, i1, j0, j1) = r.span(lo, hi);
        for j in j0..=j1 {
            for i in i0..=i1 {
                let d = seg_dist(r.center(i, j), a, b);
                if d <= reach {
                    let k = Raster::idx(i, j);
                    r.flags[k] |= F_SIDEWALK;
                    if d <= hw {
                        r.flags[k] |= F_ROAD;
                    }
                }
            }
        }
    }
}

/// Waterway band: cells within `hw` of the centerline are water.
fn paint_water_band(r: &mut Raster, pts: &[[f64; 2]], hw: f64) {
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let lo = [a[0].min(b[0]) - hw, a[1].min(b[1]) - hw];
        let hi = [a[0].max(b[0]) + hw, a[1].max(b[1]) + hw];
        let (i0, i1, j0, j1) = r.span(lo, hi);
        for j in j0..=j1 {
            for i in i0..=i1 {
                if seg_dist(r.center(i, j), a, b) <= hw {
                    r.flags[Raster::idx(i, j)] |= F_WATER;
                }
            }
        }
    }
}

/// Ring area: cells inside get `inside`; cells within `margin` of the edge get `near`.
fn paint_ring(r: &mut Raster, ring: &[[f64; 2]], margin: f64, inside: u8, near: u8) {
    if ring.len() < 3 {
        return;
    }
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in ring {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    let (i0, i1, j0, j1) = r.span(
        [lo[0] - margin, lo[1] - margin],
        [hi[0] + margin, hi[1] + margin],
    );
    for j in j0..=j1 {
        for i in i0..=i1 {
            let c = r.center(i, j);
            let k = Raster::idx(i, j);
            if inside_ring(c, ring) {
                r.flags[k] |= inside;
            } else if near != 0 && ring_dist(c, ring) <= margin {
                r.flags[k] |= near;
            }
        }
    }
}

fn rasterize(win: &Window) -> Raster {
    let origin = [
        (win.tile.0 as f64 - 1.0) * TILE_M,
        (win.tile.1 as f64 - 1.0) * TILE_M,
    ];
    let mut r = Raster {
        flags: vec![0; WIN_N * WIN_N],
        label: vec![0; WIN_N * WIN_N],
        origin,
    };
    for (_, road) in &win.roads {
        // Every road blocks the lots over its own width; only urban roads add the sidewalk band.
        let hw = road_width_m(&road.highway) / 2.0;
        let reach = if is_urban(&road.highway, polyline_len(&road.points)) {
            hw + SIDEWALK_M
        } else {
            hw
        };
        paint_band(&mut r, &road.points, hw, reach);
    }
    for ring in &win.footprints {
        paint_ring(&mut r, ring, FOOTPRINT_MARGIN_M, F_FOOT | F_BLOCK, F_BLOCK);
    }
    for (half_w, line) in &win.water {
        paint_water_band(&mut r, line, *half_w);
    }
    for (kind, ring) in &win.areas {
        let flag = match kind {
            AreaKind::Park => F_PARK,
            AreaKind::Water => F_WATER,
        };
        paint_ring(&mut r, ring, 0.0, flag, 0);
    }
    r
}

/// Labels the free components (blocks). A component that touches the window border is not
/// complete, so it gets label 0. Returns the stable id of each label (label = index + 1).
fn label_blocks(r: &mut Raster, tile: (i32, i32)) -> Vec<u64> {
    let gi0 = (tile.0 as i64 - 1) * TILE_CELLS as i64;
    let gj0 = (tile.1 as i64 - 1) * TILE_CELLS as i64;
    let mut ids = Vec::new();
    let mut stack = Vec::new();
    let mut comp = Vec::new();
    for start in 0..r.flags.len() {
        if r.flags[start] & BLOCKING != 0 || r.label[start] != 0 {
            continue;
        }
        r.label[start] = u32::MAX;
        stack.push(start);
        comp.clear();
        let mut touches = false;
        let mut key = (i64::MAX, i64::MAX);
        while let Some(c) = stack.pop() {
            comp.push(c);
            let (i, j) = (c % WIN_N, c / WIN_N);
            if i == 0 || j == 0 || i == WIN_N - 1 || j == WIN_N - 1 {
                touches = true;
            }
            key = key.min((i as i64 + gi0, j as i64 + gj0));
            let mut next = Vec::with_capacity(4);
            if i > 0 {
                next.push(c - 1);
            }
            if i + 1 < WIN_N {
                next.push(c + 1);
            }
            if j > 0 {
                next.push(c - WIN_N);
            }
            if j + 1 < WIN_N {
                next.push(c + WIN_N);
            }
            for n in next {
                if r.flags[n] & BLOCKING == 0 && r.label[n] == 0 {
                    r.label[n] = u32::MAX;
                    stack.push(n);
                }
            }
        }
        if !touches {
            ids.push(hash(&[key.0 as u64, key.1 as u64]));
            let lab = ids.len() as u32;
            for &c in &comp {
                r.label[c] = lab;
            }
        }
    }
    for l in r.label.iter_mut() {
        if *l == u32::MAX {
            *l = 0;
        }
    }
    ids
}

/// Point and unit tangent at arc length `s` of a polyline (`acc` = cumulative lengths).
fn point_at(pts: &[[f64; 2]], acc: &[f64], s: f64) -> ([f64; 2], [f64; 2]) {
    let mut k = 0;
    while k + 2 < pts.len() && acc[k + 1] < s {
        k += 1;
    }
    let (a, b) = (pts[k], pts[k + 1]);
    let len = acc[k + 1] - acc[k];
    let t = if len > 0.0 {
        ((s - acc[k]) / len).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    let d = [b[0] - a[0], b[1] - a[1]];
    let dl = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-12);
    (p, [d[0] / dl, d[1] / dl])
}

fn cumulative(pts: &[[f64; 2]]) -> Vec<f64> {
    let mut acc = vec![0.0];
    for w in pts.windows(2) {
        let last = *acc.last().unwrap_or(&0.0);
        acc.push(last + seg_len(w[0], w[1]));
    }
    acc
}

/// Accepts the lot rectangle `o + u*a + v*b` (0 <= a <= w, 0 <= b <= d) if every cell inside is
/// free, in one block and not used by another lot. Marks its cells as lot. Returns the label.
fn place(r: &mut Raster, o: [f64; 2], u: [f64; 2], v: [f64; 2], w: f64, d: f64) -> Option<u32> {
    let far = add(add(o, scale(u, w)), scale(v, d));
    let corners = [o, add(o, scale(u, w)), far, add(o, scale(v, d))];
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in corners {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    r.cell_of(lo)?;
    r.cell_of(hi)?;
    let (i0, i1, j0, j1) = r.span(lo, hi);
    let mut label = None;
    let mut cells = Vec::new();
    for j in j0..=j1 {
        for i in i0..=i1 {
            let rel = [r.center(i, j)[0] - o[0], r.center(i, j)[1] - o[1]];
            let (a, b) = (dot(rel, u), dot(rel, v));
            if a < 0.0 || a > w || b < 0.0 || b > d {
                continue;
            }
            let k = Raster::idx(i, j);
            let lab = r.label[k];
            if lab == 0 || r.flags[k] & (BLOCKING | F_LOT) != 0 {
                return None;
            }
            match label {
                None => label = Some(lab),
                Some(l) if l != lab => return None,
                _ => {}
            }
            cells.push(k);
        }
    }
    if cells.is_empty() {
        return None;
    }
    for k in cells {
        r.flags[k] |= F_LOT;
    }
    label
}

/// Building inside an accepted lot: setbacks, floors and palette, all from the lot's keys.
fn building(lot: &Lot) -> Option<Polygon> {
    let mut g = Rng(hash(&[lot.block, lot.ri as u64, lot.side as u64, lot.k, 2]));
    if g.next_u64() % 1000 < EMPTY_LOT_PERMILLE {
        return None;
    }
    let front = snap(g.unit() * FRONT_SETBACK_MAX_M);
    let left = snap(g.unit() * SIDE_SETBACK_MAX_M);
    let right = snap(g.unit() * SIDE_SETBACK_MAX_M);
    let back = snap(g.unit() * SIDE_SETBACK_MAX_M);
    let tall_p = if lot.main {
        TALL_MAIN_PERMILLE
    } else {
        TALL_PERMILLE
    };
    let levels = if g.next_u64() % 1000 < tall_p {
        (4 + (g.unit() * 5.0) as u8).min(8)
    } else {
        (1 + (g.unit() * 3.0) as u8).min(3)
    };
    let color_idx = ((g.unit() * 4.0) as u8).min(3);
    let tint = ((g.unit() * 8.0) as u8).min(7);
    let at = |a: f64, b: f64| add(add(lot.origin, scale(lot.u, a)), scale(lot.v, b));
    let (x0, x1) = (left, lot.w - right);
    let (y0, y1) = (front, lot.d - back);
    if x1 - x0 < MODULE_M || y1 - y0 < MODULE_M {
        return None;
    }
    // Corners rounded to 1 mm: far below a module, and it keeps the tile JSON small.
    let r3 = |p: [f64; 2]| {
        [
            (p[0] * 1000.0).round() / 1000.0,
            (p[1] * 1000.0).round() / 1000.0,
        ]
    };
    Some(Polygon {
        points: [at(x0, y0), at(x1, y0), at(x1, y1), at(x0, y1)]
            .into_iter()
            .map(r3)
            .collect(),
        generated: true,
        levels,
        color_idx,
        tint,
    })
}

/// Generates the lots of the window and the ground cover of its center tile.
pub fn bake_window(win: &Window) -> Output {
    let mut r = rasterize(win);
    let ids = label_blocks(&mut r, win.tile);
    let mut roads: Vec<(usize, &Polyline)> = win
        .roads
        .iter()
        .filter(|(_, p)| is_urban(&p.highway, polyline_len(&p.points)))
        .copied()
        .collect();
    roads.sort_by_key(|(ri, _)| *ri);
    let mut lots: Vec<Lot> = Vec::new();
    for (ri, road) in roads {
        let hw = road_width_m(&road.highway) / 2.0;
        let reach = hw + SIDEWALK_M;
        let acc = cumulative(&road.points);
        let total = *acc.last().unwrap_or(&0.0);
        for side in 0..2usize {
            let sign = if side == 0 { 1.0 } else { -1.0 };
            let mut s = 0.0;
            let mut k = 0u64;
            loop {
                let mut g = Rng(hash(&[ri as u64, side as u64, k]));
                let w = snap(LOT_W_MIN_M + g.unit() * (LOT_W_MAX_M - LOT_W_MIN_M));
                let d = snap(LOT_D_MIN_M + g.unit() * (LOT_D_MAX_M - LOT_D_MIN_M));
                if s + w > total {
                    break;
                }
                let (p, t) = point_at(&road.points, &acc, s);
                let v = [-t[1] * sign, t[0] * sign];
                let origin = add(p, scale(v, reach));
                if let Some(lab) = place(&mut r, origin, t, v, w, d) {
                    lots.push(Lot {
                        block: ids[lab as usize - 1],
                        ri,
                        side,
                        k,
                        origin,
                        u: t,
                        v,
                        w,
                        d,
                        main: matches!(road.highway.as_str(), "primary" | "secondary"),
                    });
                }
                s += w;
                k += 1;
            }
        }
    }
    let (tx0, ty0) = (win.tile.0 as f64 * TILE_M, win.tile.1 as f64 * TILE_M);
    let buildings = lots
        .iter()
        .filter(|lot| {
            let c = add(
                add(lot.origin, scale(lot.u, lot.w / 2.0)),
                scale(lot.v, lot.d / 2.0),
            );
            c[0] >= tx0 && c[0] < tx0 + TILE_M && c[1] >= ty0 && c[1] < ty0 + TILE_M
        })
        .filter_map(building)
        .collect();
    let mut ground = vec![GROUND_GRASS; GROUND_N * GROUND_N];
    for gy in 0..GROUND_N {
        for gx in 0..GROUND_N {
            let (mut cement, mut walk, mut water, mut park) = (false, false, false, false);
            for dj in 0..CELLS_PER_GROUND {
                for di in 0..CELLS_PER_GROUND {
                    let i = TILE_CELLS + gx * CELLS_PER_GROUND + di;
                    let j = TILE_CELLS + gy * CELLS_PER_GROUND + dj;
                    let f = r.flags[Raster::idx(i, j)];
                    cement |= f & (F_FOOT | F_LOT) != 0;
                    walk |= f & F_SIDEWALK != 0 && f & F_ROAD == 0;
                    water |= f & F_WATER != 0;
                    park |= f & F_PARK != 0;
                }
            }
            ground[gy * GROUND_N + gx] = if cement {
                GROUND_CONCRETE
            } else if walk {
                GROUND_SIDEWALK
            } else if water {
                GROUND_WATER
            } else if park {
                GROUND_PARK
            } else {
                GROUND_GRASS
            };
        }
    }
    Output { buildings, ground }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tile (0,0) with a closed block (x -44.5..174.5, y 55.5..194.5) bounded by two
    /// horizontal and two vertical residential streets, and one real building inside it.
    fn synthetic<'a>(roads: &'a [Polyline], foot: &'a [[f64; 2]]) -> Window<'a> {
        Window {
            tile: (0, 0),
            roads: roads.iter().enumerate().collect(),
            footprints: vec![foot],
            areas: Vec::new(),
            water: Vec::new(),
        }
    }

    fn street(points: &[[f64; 2]]) -> Polyline {
        Polyline {
            name: String::new(),
            highway: "residential".into(),
            points: points.to_vec(),
            z: vec![],
            zl: vec![],
            zr: vec![],
            lit: None,
        }
    }

    fn roads() -> Vec<Polyline> {
        vec![
            street(&[[-240.0, 50.0], [480.0, 50.0]]),
            street(&[[-240.0, 200.0], [480.0, 200.0]]),
            street(&[[-50.0, -240.0], [-50.0, 480.0]]),
            street(&[[180.0, -240.0], [180.0, 480.0]]),
        ]
    }

    const FOOT: [[f64; 2]; 4] = [[60.0, 100.0], [80.0, 100.0], [80.0, 120.0], [60.0, 120.0]];

    fn edge_len(p: &Polygon, k: usize) -> f64 {
        let (a, b) = (p.points[k], p.points[(k + 1) % p.points.len()]);
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
    }

    #[test]
    fn synthetic_tile_gets_lots_and_concrete() {
        let rs = roads();
        let win = synthetic(&rs, &FOOT);
        let out = bake_window(&win);
        assert!(out.buildings.len() >= 10, "lots: {}", out.buildings.len());
        // Ground under the real footprint is concrete: cell at (70, 110) m in tile (0,0).
        let gi = (70.0 / GROUND_CELL_M) as usize;
        let gj = (110.0 / GROUND_CELL_M) as usize;
        assert_eq!(out.ground[gj * GROUND_N + gi], GROUND_CONCRETE);
        assert!(out.ground.iter().filter(|&&g| g == GROUND_CONCRETE).count() > 100);
    }

    #[test]
    fn lots_avoid_real_building_roads_and_areas() {
        let rs = roads();
        let park = [[120.0, 90.0], [150.0, 90.0], [150.0, 110.0], [120.0, 110.0]];
        let mut win = synthetic(&rs, &FOOT);
        win.areas = vec![(AreaKind::Park, &park[..])];
        let r = rasterize(&win);
        assert!(
            r.flags.iter().any(|f| f & F_PARK != 0),
            "park was not rasterized"
        );
        let out = bake_window(&win);
        for b in &out.buildings {
            let (lo, hi) = b
                .points
                .iter()
                .fold(([f64::MAX; 2], [f64::MIN; 2]), |(l, h), p| {
                    (
                        [l[0].min(p[0]), l[1].min(p[1])],
                        [h[0].max(p[0]), h[1].max(p[1])],
                    )
                });
            let (i0, i1, j0, j1) = r.span(lo, hi);
            for j in j0..=j1 {
                for i in i0..=i1 {
                    let c = r.center(i, j);
                    if inside_ring(c, &b.points) {
                        assert_eq!(
                            r.flags[Raster::idx(i, j)] & BLOCKING,
                            0,
                            "building over blocked cell {c:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn sizes_are_multiples_of_half_meter() {
        let rs = roads();
        let out = bake_window(&synthetic(&rs, &FOOT));
        for b in &out.buildings {
            for k in 0..4 {
                let q = edge_len(b, k) / MODULE_M;
                // 1 mm rounding of the corners moves an edge by at most 2 mm.
                assert!(
                    (q - q.round()).abs() * MODULE_M < 2e-3,
                    "edge {} m",
                    edge_len(b, k)
                );
            }
            assert!((1..=8).contains(&b.levels) && b.generated && b.color_idx <= 3);
        }
    }

    #[test]
    fn rebake_is_identical() {
        let rs = roads();
        let a = bake_window(&synthetic(&rs, &FOOT));
        let b = bake_window(&synthetic(&rs, &FOOT));
        assert_eq!(a, b);
    }

    #[test]
    fn neighbour_tiles_share_the_same_lots_without_repeats() {
        // Block x 25.5..464.5 fits in the window of tiles (0,0) and (1,0): both bake it.
        let rs = vec![
            street(&[[-100.0, 50.0], [600.0, 50.0]]),
            street(&[[-100.0, 200.0], [600.0, 200.0]]),
            street(&[[20.0, -100.0], [20.0, 300.0]]),
            street(&[[470.0, -100.0], [470.0, 300.0]]),
        ];
        let all: Vec<(usize, &Polyline)> = rs.iter().enumerate().collect();
        let mk = |t: (i32, i32)| Window {
            tile: t,
            roads: all.clone(),
            footprints: vec![],
            areas: vec![],
            water: vec![],
        };
        let left = bake_window(&mk((0, 0)));
        let right = bake_window(&mk((1, 0)));
        let mut seen = std::collections::HashSet::new();
        for b in left.buildings.iter().chain(right.buildings.iter()) {
            let key: Vec<(i64, i64)> = b
                .points
                .iter()
                .map(|p| ((p[0] * 1000.0) as i64, (p[1] * 1000.0) as i64))
                .collect();
            assert!(seen.insert(key), "a lot was repeated");
        }
        assert!(!left.buildings.is_empty() && !right.buildings.is_empty());
    }
}
