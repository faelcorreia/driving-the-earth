//! Map logic without Godot types: loads a baked map from `cache/<id>/` and turns
//! each tile into plain triangle meshes, so it can be unit-tested with `cargo test`.
//!
//! Coordinates: the tile plane is local meters, `x` east and `y` north, with the
//! home (map center) at the origin. Godot world space is `x` east, `y` up and
//! `z = -north`, so the car's forward (-Z) points north.
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tilebake::profile::z_at;
use tileformat::{read_tile, MapMeta, Polyline, Tile};

/// Height of the road ribbons above the terrain, in meters (avoids z-fighting).
const ROAD_LIFT_M: f32 = 0.1;
/// Extra lift per road index (modulo 8), so overlapping ribbons do not z-fight.
const ROAD_ORDER_LIFT_M: f32 = 0.002;
/// How far from another road's band an end still counts as a junction, in meters.
const JUNCTION_TOL_M: f64 = 1.0;
/// How far a road end on a tile border runs past it, in meters, so the ribbons of the two
/// tiles overlap and no hairline of terrain shows through the seam.
const BORDER_OVERLAP_M: f64 = 0.25;

/// A baked map: its metadata and every tile found in `cache/<id>/tiles/`.
pub struct Map {
    pub meta: MapMeta,
    /// Sorted by `(x, y)`.
    pub tiles: Vec<Tile>,
    index: HashMap<(i32, i32), usize>,
}

/// Triangle mesh in Godot world space. `indices` takes three at a time.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MeshData {
    pub vertices: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

impl MeshData {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// The triangles as a flat list of points (three per triangle), for concave collision.
    pub fn triangle_points(&self) -> Vec<[f32; 3]> {
        self.indices.iter().map(|&i| self.vertices[i as usize]).collect()
    }
}

/// Terrain and road meshes of one tile.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TileGeometry {
    pub terrain: MeshData,
    pub roads: MeshData,
}

impl Map {
    pub fn new(meta: MapMeta, tiles: Vec<Tile>) -> Self {
        let index = tiles.iter().enumerate().map(|(i, t)| ((t.x, t.y), i)).collect();
        Map { meta, tiles, index }
    }

    /// Terrain height at local `(x, y)`, or `None` when no tile covers that point.
    pub fn height_at(&self, x: f64, y: f64) -> Option<f32> {
        let size = self.meta.tile_size_m;
        let key = ((x / size).floor() as i32, (y / size).floor() as i32);
        let tile = &self.tiles[*self.index.get(&key)?];
        Some(sample_height(tile, size, x, y))
    }

    /// Like `tile_geometry`, but roads sit on the terrain of whichever tile owns each point.
    /// A ribbon that crosses a tile border (half its width, or its overlap) then follows the
    /// neighbor's real surface instead of an extension of this tile's edge plane.
    pub fn tile_geometry(&self, tile: &Tile) -> TileGeometry {
        let size = self.meta.tile_size_m;
        tile_geometry_with(tile, size, &|x, y| self.height_at(x, y).unwrap_or_else(|| sample_height(tile, size, x, y)))
    }
}

fn invalid_data<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

/// A baked map opened without reading its tiles: the metadata and the grid coordinates of
/// the files in `cache/<id>/tiles/`. Tiles are read on demand, so opening a big map is cheap.
/// Read-only, so it can be shared with the loader thread.
pub struct MapIndex {
    pub meta: MapMeta,
    dir: PathBuf,
    /// Sorted.
    coords: Vec<(i32, i32)>,
}

/// Reads `cache_root/<id>/meta.json` and lists `tiles/x_y.json` by name, without parsing tiles.
/// Fails when the map is not complete (tilebake has not finished).
pub fn open_map(cache_root: &Path, id: &str) -> io::Result<MapIndex> {
    let dir = cache_root.join(id);
    let meta: MapMeta = serde_json::from_str(&fs::read_to_string(dir.join("meta.json"))?).map_err(invalid_data)?;
    if !meta.complete {
        return Err(invalid_data(format!("map '{id}' is not complete, run tilebake first")));
    }
    let mut coords = Vec::new();
    for entry in fs::read_dir(dir.join("tiles"))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Some(c) = path.file_stem().and_then(|s| s.to_str()).and_then(parse_coords) {
            coords.push(c);
        }
    }
    coords.sort_unstable();
    Ok(MapIndex { meta, dir, coords })
}

/// `"-3_12"` to `(-3, 12)`.
fn parse_coords(stem: &str) -> Option<(i32, i32)> {
    let (x, y) = stem.split_once('_')?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

fn check_tile(tile: &Tile) -> io::Result<()> {
    let n = tile.height.cols as usize;
    if n < 2 || tile.height.rows as usize != n || tile.height.values.len() != n * n {
        return Err(invalid_data(format!("tile {} {} has a bad height grid", tile.x, tile.y)));
    }
    Ok(())
}

impl MapIndex {
    /// Grid coordinates of every tile file, sorted.
    pub fn coords(&self) -> &[(i32, i32)] {
        &self.coords
    }

    pub fn has_tile(&self, x: i32, y: i32) -> bool {
        self.coords.binary_search(&(x, y)).is_ok()
    }

    /// Reads and validates one tile file.
    pub fn read_tile_at(&self, x: i32, y: i32) -> io::Result<Tile> {
        let tile = read_tile(&self.dir.join("tiles").join(format!("{x}_{y}.json")))?;
        check_tile(&tile)?;
        Ok(tile)
    }

    /// The existing tiles within one step of `(x, y)`, as a `Map`. Its `tile_geometry` for a
    /// center tile equals the whole map's: no road ribbon reaches past a neighbor's edge.
    pub fn neighborhood(&self, x: i32, y: i32) -> io::Result<Map> {
        let mut tiles = Vec::new();
        for ty in y - 1..=y + 1 {
            for tx in x - 1..=x + 1 {
                if self.has_tile(tx, ty) {
                    tiles.push(self.read_tile_at(tx, ty)?);
                }
            }
        }
        Ok(Map::new(self.meta.clone(), tiles))
    }

    /// Geometry of tile `(x, y)` as the whole map would give it, reading only its neighborhood.
    pub fn tile_geometry(&self, x: i32, y: i32) -> io::Result<TileGeometry> {
        let map = self.neighborhood(x, y)?;
        let center = map
            .tiles
            .iter()
            .find(|t| (t.x, t.y) == (x, y))
            .ok_or_else(|| invalid_data(format!("no tile {x} {y} in map '{}'", self.meta.id)))?;
        Ok(map.tile_geometry(center))
    }
}

/// Terrain height at local `(x, y)`, on the same triangles that `tile_geometry` draws
/// (each grid cell is split along the diagonal from its north-east to its south-west corner).
/// Sampling the bilinear surface instead would differ from the drawn mesh on slopes, so
/// anything placed "on" the terrain could end up under it.
/// Points outside the tile extrapolate the plane of the nearest edge cell, which matches the
/// neighbor tile along the shared edge. The grid has `cols` samples per side over `tile_m`
/// meters, edges included.
pub fn sample_height(tile: &Tile, tile_m: f64, x: f64, y: f64) -> f32 {
    let grid = &tile.height;
    let n = grid.cols as usize;
    let cell = tile_m / (n - 1) as f64;
    let u = (x - tile.x as f64 * tile_m) / cell;
    let v = (y - tile.y as f64 * tile_m) / cell;
    let j0 = (u.floor().max(0.0) as usize).min(n - 2);
    let i0 = (v.floor().max(0.0) as usize).min(n - 2);
    let (fu, fv) = (u - j0 as f64, v - i0 as f64);
    let at = |i: usize, j: usize| grid.values[i * n + j] as f64;
    let (a, b, c, d) = (at(i0, j0), at(i0, j0 + 1), at(i0 + 1, j0), at(i0 + 1, j0 + 1));
    let h = if fu + fv <= 1.0 {
        // Triangle a, b, c.
        a + (b - a) * fu + (c - a) * fv
    } else {
        // Triangle b, c, d.
        d + (c - d) * (1.0 - fu) + (b - d) * (1.0 - fv)
    };
    h as f32
}

/// Road width in meters by OSM `highway` class.
pub fn road_width_m(highway: &str) -> f64 {
    match highway {
        "motorway" | "trunk" | "primary" => 12.0,
        "secondary" => 10.0,
        "tertiary" => 8.0,
        "residential" | "unclassified" | "living_street" => 6.0,
        _ => 3.0,
    }
}

/// Builds the terrain grid and the road ribbons of one tile, with no neighbors (tests only;
/// the game uses `Map::tile_geometry`, which sees the neighbors).
#[cfg(test)]
pub fn tile_geometry(tile: &Tile, tile_m: f64) -> TileGeometry {
    tile_geometry_with(tile, tile_m, &|x, y| sample_height(tile, tile_m, x, y))
}

/// `tile_geometry` with the terrain height under the roads given by `ground(x, y)`.
pub fn tile_geometry_with(tile: &Tile, tile_m: f64, ground: &dyn Fn(f64, f64) -> f32) -> TileGeometry {
    let n = tile.height.cols as usize;
    let cell = tile_m / (n - 1) as f64;
    let (x0, y0) = (tile.x as f64 * tile_m, tile.y as f64 * tile_m);
    let ends = extend_junction_ends(&tile.roads, tile_m);
    let heights = lower_under_roads(&tile.height.values, n, [x0, y0], cell, &tile.roads, &ends);
    let h = |i: usize, j: usize| heights[i * n + j];

    let mut terrain = MeshData::default();
    for i in 0..n {
        for j in 0..n {
            // Normal from central differences (one-sided at the tile edges).
            let (j0, j1) = (j.saturating_sub(1), (j + 1).min(n - 1));
            let (i0, i1) = (i.saturating_sub(1), (i + 1).min(n - 1));
            let dh_dx = (h(i, j1) - h(i, j0)) as f64 / ((j1 - j0) as f64 * cell);
            let dh_dy = (h(i1, j) - h(i0, j)) as f64 / ((i1 - i0) as f64 * cell);
            terrain.vertices.push(world(
                x0 + j as f64 * cell,
                y0 + i as f64 * cell,
                h(i, j),
            ));
            terrain.normals.push(normalize([-dh_dx, 1.0, dh_dy]));
        }
    }
    for i in 0..n - 1 {
        for j in 0..n - 1 {
            let a = (i * n + j) as u32;
            let (b, c) = (a + 1, a + n as u32);
            let d = c + 1;
            // Godot front faces are clockwise seen from above, so wind them that way.
            terrain.indices.extend([a, c, b, b, c, d]);
        }
    }

    let mut roads = MeshData::default();
    // Stations every quarter of a grid cell: the ribbon follows the terrain between nodes.
    let step = cell / ROAD_STATIONS_PER_CELL;
    for (idx, road) in tile.roads.iter().enumerate() {
        let half = road_width_m(&road.highway) / 2.0;
        let lift = ROAD_LIFT_M + (idx % 8) as f32 * ROAD_ORDER_LIFT_M;
        let (pts, zs) = &ends[idx];
        push_road(&mut roads, ground, pts, zs, half, step, lift);
    }

    TileGeometry { terrain, roads }
}

/// Terrain vertices closer than this (in grid cells, beyond the half width) to a road with a
/// profile are lowered. Any triangle that touches the band then has all its corners lowered.
const LOWER_MARGIN_CELLS: f64 = 1.5;
/// Lowered terrain vertices sit this far below the road axis height (the road lies above).
const LOWER_BELOW_AXIS_M: f32 = 0.1;

/// Terrain grid heights (row-major, `n` per side, origin `origin` in map meters) with each
/// vertex near a road that has a profile lowered to `z - LOWER_BELOW_AXIS_M`, where `z` is
/// the height of the nearest axis point. Vertices never go up, and the triangles stay as
/// they are: the road ribbon then lies above the terrain where the grid resolves a crest.
fn lower_under_roads(
    values: &[f32],
    n: usize,
    origin: [f64; 2],
    cell: f64,
    roads: &[Polyline],
    ends: &[(Vec<[f64; 2]>, Vec<f32>)],
) -> Vec<f32> {
    let mut out = values.to_vec();
    for (road, (pts, zs)) in roads.iter().zip(ends) {
        if pts.len() < 2 || zs.len() != pts.len() {
            continue;
        }
        let reach = road_width_m(&road.highway) / 2.0 + LOWER_MARGIN_CELLS * cell;
        for k in 0..pts.len() - 1 {
            let (a, b) = (pts[k], pts[k + 1]);
            let (za, zb) = (zs[k], zs[k + 1]);
            let d = [b[0] - a[0], b[1] - a[1]];
            let len2 = d[0] * d[0] + d[1] * d[1];
            // Index range of the grid vertices in the segment's box, grown by `reach`.
            let range = |lo: f64, hi: f64, o: f64| {
                let i0 = ((lo - reach - o) / cell).floor().max(0.0) as usize;
                let i1 = (((hi + reach - o) / cell).ceil().max(0.0) as usize).min(n - 1);
                (i0, i1)
            };
            let (j0, j1) = range(a[0].min(b[0]), a[0].max(b[0]), origin[0]);
            let (i0, i1) = range(a[1].min(b[1]), a[1].max(b[1]), origin[1]);
            for i in i0..=i1 {
                for j in j0..=j1 {
                    let p = [origin[0] + j as f64 * cell, origin[1] + i as f64 * cell];
                    let t = if len2 > 0.0 {
                        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len2).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let q = [a[0] + d[0] * t, a[1] + d[1] * t];
                    if (p[0] - q[0]).hypot(p[1] - q[1]) > reach {
                        continue;
                    }
                    let cand = za + (zb - za) * t as f32 - LOWER_BELOW_AXIS_M;
                    let v = &mut out[i * n + j];
                    if cand < *v {
                        *v = cand;
                    }
                }
            }
        }
    }
    out
}

/// Stations per terrain grid cell along a road.
const ROAD_STATIONS_PER_CELL: f64 = 4.0;
/// Miter joins are cut back when the corner is sharper than this (scale limit).
const MITER_LIMIT: f64 = 2.0;

/// Adds one road as a continuous strip: three vertices per station (left edge, center,
/// right edge), two quads between stations. Stations sit at the polyline nodes and every
/// `step` meters in between, so each vertex is sampled on the terrain and the strip
/// does not cut under a crest between two nodes. At each node the two segment normals
/// are joined (miter), so bends have no gaps or overlaps.
fn push_road(mesh: &mut MeshData, ground: &dyn Fn(f64, f64) -> f32, points: &[[f64; 2]], zs: &[f32], half: f64, step: f64, lift: f32) {
    // Drop repeated points (and their heights): they have no direction.
    let mut pts: Vec<[f64; 2]> = Vec::with_capacity(points.len());
    let mut heights: Vec<f32> = Vec::with_capacity(zs.len());
    for (k, &p) in points.iter().enumerate() {
        match pts.last() {
            Some(&q) if dist(p, q) <= 1e-9 => {}
            _ => {
                pts.push(p);
                if let Some(&z) = zs.get(k) {
                    heights.push(z);
                }
            }
        }
    }
    if pts.len() < 2 {
        return;
    }
    let has_z = heights.len() == pts.len();
    let normals: Vec<[f64; 2]> = pts.windows(2).map(|w| unit_normal(w[0], w[1])).collect();

    // Offset from the center to the left edge, one per station, with its position and,
    // when the bake gave a profile, the height of the axis there.
    let mut stations: Vec<([f64; 2], [f64; 2], Option<f32>)> = Vec::new();
    stations.push((pts[0], scale(normals[0], half), has_z.then(|| heights[0])));
    for k in 0..pts.len() - 1 {
        let (p, q) = (pts[k], pts[k + 1]);
        let pieces = (dist(p, q) / step).ceil().max(1.0) as usize;
        for s in 1..pieces {
            let t = s as f64 / pieces as f64;
            let at = [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t];
            let z = has_z.then(|| heights[k] + (heights[k + 1] - heights[k]) * t as f32);
            stations.push((at, scale(normals[k], half), z));
        }
        let offset = match normals.get(k + 1) {
            Some(&next) => miter(normals[k], next, half),
            None => scale(normals[k], half),
        };
        stations.push((q, offset, has_z.then(|| heights[k + 1])));
    }

    let base = mesh.vertices.len() as u32;
    for &(c, o, z) in &stations {
        for [x, y] in [[c[0] + o[0], c[1] + o[1]], c, [c[0] - o[0], c[1] - o[1]]] {
            let height = match z {
                Some(z) => z + lift,
                None => ground(x, y) + lift,
            };
            mesh.vertices.push(world(x, y, height));
            mesh.normals.push([0.0, 1.0, 0.0]);
        }
    }
    for s in 0..stations.len() - 1 {
        // Vertex of station s on the left (3s), center (3s + 1) and right (3s + 2) edge.
        let l0 = base + 3 * s as u32;
        let l1 = l0 + 3;
        push_up_facing(mesh, l0, l0 + 1, l1 + 1);
        push_up_facing(mesh, l0, l1 + 1, l1);
        push_up_facing(mesh, l0 + 1, l0 + 2, l1 + 2);
        push_up_facing(mesh, l0 + 1, l1 + 2, l1 + 1);
    }
}

/// Points of every road of a tile, with each end that meets another road extended so that
/// its cut lies past the near edge of the crossed road. Ends on a tile border run a
/// little past it (overlap with the next tile); ends with no other road close by
/// (cul-de-sacs) are kept as they are.
/// Each road's points with their heights (empty when the bake gave no profile). Points added
/// by an extension take the height of the nearest point of the original road.
fn extend_junction_ends(roads: &[Polyline], tile_m: f64) -> Vec<(Vec<[f64; 2]>, Vec<f32>)> {
    roads
        .iter()
        .enumerate()
        .map(|(i, road)| {
            let half = road_width_m(&road.highway) / 2.0;
            let mut pts = road.points.clone();
            if pts.len() < 2 {
                return (pts, Vec::new());
            }
            let head = extend_end(&pts, half, roads, i, tile_m);
            extend_tail(&mut pts, head);
            pts.reverse();
            let tail = extend_end(&pts, half, roads, i, tile_m);
            extend_tail(&mut pts, tail);
            pts.reverse();
            let has_z = road.z.len() == road.points.len();
            let zs = if has_z {
                pts.iter().map(|&p| z_at(&road.points, &road.z, p)).collect()
            } else {
                Vec::new()
            };
            (pts, zs)
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

/// The last point before `end` in `pts` that differs from it (or `end` if none).
fn last_distinct(pts: &[[f64; 2]], end: [f64; 2]) -> [f64; 2] {
    pts.iter().rev().copied().find(|&p| dist(p, end) > 1e-9).unwrap_or(end)
}

/// Unit vector from `p` to `q`. Caller ensures `p != q`.
fn unit(p: [f64; 2], q: [f64; 2]) -> [f64; 2] {
    let len = dist(p, q);
    [(q[0] - p[0]) / len, (q[1] - p[1]) / len]
}

/// Distance from `p` to the segment `a -> b`.
fn dist_to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    dist(p, [a[0] + t * dx, a[1] + t * dy])
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
}

fn scale(v: [f64; 2], k: f64) -> [f64; 2] {
    [v[0] * k, v[1] * k]
}

/// Unit normal (left of the direction) of the segment `p -> q`. Caller ensures `p != q`.
fn unit_normal(p: [f64; 2], q: [f64; 2]) -> [f64; 2] {
    let len = dist(p, q);
    [-(q[1] - p[1]) / len, (q[0] - p[0]) / len]
}

/// Offset of length `half` at a node joining segments with normals `m1` and `m2`:
/// the bisector scaled so both edges stay at `half` from the centerline.
fn miter(m1: [f64; 2], m2: [f64; 2], half: f64) -> [f64; 2] {
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

/// Local `(x, y)` plus height to Godot world space.
fn world(x: f64, y: f64, height: f32) -> [f32; 3] {
    [x as f32, height, -(y as f32)]
}

fn normalize(v: [f64; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [(v[0] / len) as f32, (v[1] / len) as f32, (v[2] / len) as f32]
}

/// Adds the triangle with its winding fixed so that its front faces up.
/// Godot front faces are clockwise seen from above, i.e. the y component of
/// `(b - a) x (c - a)` is negative.
fn push_up_facing(mesh: &mut MeshData, a: u32, b: u32, c: u32) {
    let (pa, pb, pc) = (mesh.vertices[a as usize], mesh.vertices[b as usize], mesh.vertices[c as usize]);
    // y component of (b - a) x (c - a)
    let cross_y = (pb[2] - pa[2]) * (pc[0] - pa[0]) - (pb[0] - pa[0]) * (pc[2] - pa[2]);
    if cross_y <= 0.0 {
        mesh.indices.extend([a, b, c]);
    } else {
        mesh.indices.extend([a, c, b]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tileformat::{write_tile, HeightGrid, Polyline};

    fn grid(cols: u32, f: impl Fn(f64, f64) -> f32) -> HeightGrid {
        let last = (cols - 1) as f64;
        let mut values = Vec::new();
        for i in 0..cols {
            for j in 0..cols {
                values.push(f(j as f64 / last, i as f64 / last));
            }
        }
        HeightGrid {
            cols,
            rows: cols,
            values,
        }
    }

    fn tile(x: i32, y: i32, height: HeightGrid, roads: Vec<Polyline>) -> Tile {
        Tile {
            x,
            y,
            height,
            roads,
            buildings: Vec::new(),
        }
    }

    fn meta(complete: bool) -> MapMeta {
        MapMeta {
            id: "t".into(),
            name: "t".into(),
            center_lat: 0.0,
            center_lon: 0.0,
            radius_m: 1000.0,
            tile_size_m: 20.0,
            created_unix: 0,
            complete,
        }
    }

    /// Reads every tile of a map eagerly (tests only).
    fn load_map(root: &Path, id: &str) -> io::Result<Map> {
        let index = open_map(root, id)?;
        let tiles = index
            .coords()
            .iter()
            .map(|&(x, y)| index.read_tile_at(x, y))
            .collect::<io::Result<Vec<_>>>()?;
        Ok(Map::new(index.meta.clone(), tiles))
    }

    fn temp_cache_root(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("gdext-map-{tag}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn sample_height_is_linear_on_a_ramp() {
        // 3x3 samples over 20 m (cell 10 m): height = x_m + 100 (ramp along x).
        let t = tile(0, 0, grid(3, |u, _| (u * 20.0) as f32 + 100.0), Vec::new());
        assert!((sample_height(&t, 20.0, 0.0, 0.0) - 100.0).abs() < 1e-4);
        assert!((sample_height(&t, 20.0, 5.0, 7.0) - 105.0).abs() < 1e-4);
        assert!((sample_height(&t, 20.0, 20.0, 3.0) - 120.0).abs() < 1e-4);
        // Outside the tile: the edge plane is extended (the neighbor has the same edge).
        assert!((sample_height(&t, 20.0, 30.0, -9.0) - 130.0).abs() < 1e-4);
    }

    #[test]
    fn sample_height_follows_the_mesh_diagonal() {
        // One cell (2x2 samples, 10 m): only the north-east corner is high.
        // The mesh splits the cell from north-east to south-west, so the south-west
        // half is flat and the point (5, 5) is at 0 m (bilinear would give 2.5 m).
        let t = tile(0, 0, HeightGrid { cols: 2, rows: 2, values: vec![0.0, 0.0, 0.0, 10.0] }, Vec::new());
        assert!((sample_height(&t, 10.0, 5.0, 5.0)).abs() < 1e-5);
        // In the north-east half: d + ... = 4 m at (7, 7).
        assert!((sample_height(&t, 10.0, 7.0, 7.0) - 4.0).abs() < 1e-4);
        // Mesh vertices give their own heights.
        assert!((sample_height(&t, 10.0, 10.0, 10.0) - 10.0).abs() < 1e-5);
    }

    /// Smooth hill of 8 m, 20 m wide, centered in a 250 m tile.
    fn hill(x: f64, y: f64) -> f32 {
        (8.0 * (-((x - 125.0).powi(2) + (y - 125.0).powi(2)) / (2.0 * 20.0 * 20.0)).exp()) as f32
    }

    /// Height of the drawn road mesh at local `(x, y)`, by barycentric interpolation
    /// over the triangle that contains the point (`None` outside all triangles).
    fn ribbon_height(mesh: &MeshData, x: f64, y: f64) -> Option<f64> {
        for tri in mesh.indices.chunks(3) {
            let p: Vec<[f64; 3]> = tri
                .iter()
                .map(|&i| {
                    let v = mesh.vertices[i as usize];
                    // Godot (x, y, z) -> local (x, y) with z = -north.
                    [v[0] as f64, v[1] as f64, -(v[2] as f64)]
                })
                .collect();
            let det = (p[1][0] - p[0][0]) * (p[2][2] - p[0][2]) - (p[2][0] - p[0][0]) * (p[1][2] - p[0][2]);
            if det.abs() < 1e-12 {
                continue;
            }
            let l1 = ((x - p[0][0]) * (p[2][2] - p[0][2]) - (p[2][0] - p[0][0]) * (y - p[0][2])) / det;
            let l2 = ((p[1][0] - p[0][0]) * (y - p[0][2]) - (x - p[0][0]) * (p[1][2] - p[0][2])) / det;
            let l0 = 1.0 - l1 - l2;
            let eps = -1e-9;
            if l0 >= eps && l1 >= eps && l2 >= eps {
                return Some(l0 * p[0][1] + l1 * p[1][1] + l2 * p[2][1]);
            }
        }
        None
    }

    #[test]
    fn road_never_goes_under_the_terrain() {
        // A bendy road over a hill: the strip is sampled between nodes, so it must stay
        // above the drawn terrain everywhere, not only at the nodes.
        let road = Polyline { z: Vec::new(),
            highway: "residential".into(),
            points: vec![[60.0, 100.0], [120.0, 128.0], [190.0, 150.0], [200.0, 60.0]],
        };
        let t = tile(0, 0, grid(26, |u, v| hill(u * 250.0, v * 250.0)), vec![road.clone()]);
        let g = tile_geometry(&t, 250.0);
        let mut checked = 0;
        for k in 0..road.points.len() - 1 {
            let (p, q) = (road.points[k], road.points[k + 1]);
            for s in 0..=40 {
                for side in [-3.0, -1.5, 0.0, 1.5, 3.0] {
                    let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
                    let len = (dx * dx + dy * dy).sqrt();
                    let t_ = s as f64 / 40.0;
                    // Stay off the nodes of the polyline: the bend is covered by the miter.
                    let x = p[0] + dx * t_ + -dy / len * side;
                    let y = p[1] + dy * t_ + dx / len * side;
                    let Some(strip) = ribbon_height(&g.roads, x, y) else { continue };
                    let terrain = sample_height(&t, 250.0, x, y) as f64;
                    // ROAD_LIFT_M covers the curvature between stations, but this synthetic hill is much
                    // steeper than real terrain, so allow a few centimeters.
                    assert!(strip >= terrain - 0.05, "road under terrain at ({x:.2}, {y:.2}): {strip} < {terrain}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 400, "only {checked} points checked");
    }

    #[test]
    fn road_with_profile_follows_its_heights() {
        // Flat terrain at 0 m, road heights ramp 0 -> 10 m: the ribbon follows the profile.
        let road = Polyline {
            highway: "residential".into(),
            points: vec![[60.0, 125.0], [190.0, 125.0]],
            z: vec![0.0, 10.0],
        };
        let t = tile(0, 0, grid(26, |_, _| 0.0), vec![road]);
        let g = tile_geometry(&t, 250.0);
        for x in [70.0, 125.0, 180.0] {
            let strip = ribbon_height(&g.roads, x, 125.0).expect("road covers the point");
            let want = (x - 60.0) / 130.0 * 10.0 + 0.1;
            assert!((strip - want).abs() < 0.01, "x={x}: {strip} != {want}");
        }
    }

    #[test]
    fn terrain_under_a_profiled_road_is_lowered_not_raised() {
        // A crest of 2 m across the road at x = 125; the road profile is flat at 0 m.
        let crest = |u: f64, _v: f64| {
            let x = u * 250.0;
            (2.0 * (-((x - 125.0) / 6.0).powi(2)).exp()) as f32
        };
        let road = Polyline {
            highway: "residential".into(),
            points: vec![[0.0, 125.0], [250.0, 125.0]],
            z: vec![0.0, 0.0],
        };
        let raw = tile(0, 0, grid(26, crest), Vec::new());
        let with = tile(0, 0, grid(26, crest), vec![road.clone()]);
        let plain = tile_geometry(&raw, 250.0);
        let g = tile_geometry(&with, 250.0);
        assert_eq!(g.terrain.indices, plain.terrain.indices, "triangles must not change");
        let cell = 250.0 / 25.0;
        let mut lowered = 0;
        for (v, w) in g.terrain.vertices.iter().zip(&plain.terrain.vertices) {
            let (x, y) = (v[0] as f64, -(v[2] as f64));
            let dist = (y - 125.0).abs();
            if dist <= 3.0 + 1.5 * cell {
                assert!(v[1] <= -0.1 + 1e-4, "vertex ({x:.1}, {y:.1}) still at {}", v[1]);
                lowered += usize::from(v[1] < w[1] - 1e-6);
            } else {
                assert_eq!(v[1], w[1], "vertex outside the band changed at ({x:.1}, {y:.1})");
            }
        }
        assert!(lowered > 0, "the crest was not lowered");
        // The ribbon lies above the (lowered) terrain at the crest.
        let strip = ribbon_height(&g.roads, 125.0, 125.0).expect("road covers the crest");
        assert!((strip - 0.1).abs() < 0.01, "ribbon at {strip}");
    }

    #[test]
    fn road_ends_meet_across_tiles() {
        // Two tiles share the edge x = 250 m: a road crossing it gives a piece on each side.
        // Their end vertices are the same point, with the same height from both tiles.
        // Grid values are global heights: tile (0, 0) spans x in [0, 250], tile (1, 0) in [250, 500].
        let left = tile(0, 0, grid(26, |u, v| hill(u * 250.0, v * 250.0)), vec![]);
        let right = tile(1, 0, grid(26, |u, v| hill(250.0 + u * 250.0, v * 250.0)), vec![]);
        for y in [0.0, 60.0, 125.0, 200.0] {
            let a = sample_height(&left, 250.0, 250.0, y);
            let b = sample_height(&right, 250.0, 250.0, y);
            assert!((a - b).abs() < 1e-4, "edge height differs at y={y}: {a} vs {b}");
        }
        // The strip of each tile ends on the shared border line.
        let piece = |x0: f64, x1: f64| Polyline { z: Vec::new(),
            highway: "residential".into(),
            points: vec![[x0, 125.0], [x1, 125.0]],
        };
        let gl = tile_geometry(&tile(0, 0, grid(26, |u, v| hill(u * 250.0, v * 250.0)), vec![piece(100.0, 250.0)]), 250.0);
        let gr = tile_geometry(
            &tile(1, 0, grid(26, |u, v| hill(250.0 + u * 250.0, v * 250.0)), vec![piece(250.0, 400.0)]),
            250.0,
        );
        let edge_x = |m: &MeshData| {
            m.vertices.iter().filter(|v| (v[0] - 250.0).abs() < 1e-4).map(|v| (v[1], v[2])).collect::<Vec<_>>()
        };
        assert!(!edge_x(&gl.roads).is_empty() && !edge_x(&gr.roads).is_empty());
        for (y1, z1) in edge_x(&gl.roads) {
            assert!(edge_x(&gr.roads).iter().any(|&(y2, z2)| (y1 - y2).abs() < 1e-4 && (z1 - z2).abs() < 1e-4));
        }
    }

    #[test]
    fn sample_height_uses_tile_offset() {
        // Tile (1, -1) starts at x = 20 m, y = -20 m.
        let t = tile(1, -1, grid(3, |_, _| 7.5), Vec::new());
        assert!((sample_height(&t, 20.0, 30.0, -10.0) - 7.5).abs() < 1e-6);
    }

    #[test]
    fn terrain_is_a_grid_with_front_faces_up() {
        let t = tile(0, 0, grid(5, |u, v| (u * 3.0 + v * 2.0) as f32), Vec::new());
        let g = tile_geometry(&t, 20.0);
        assert_eq!(g.terrain.vertices.len(), 25);
        assert_eq!(g.terrain.normals.len(), 25);
        assert_eq!(g.terrain.indices.len(), 2 * 4 * 4 * 3);
        assert!(g.terrain.indices.iter().all(|&i| (i as usize) < 25));
        for tri in g.terrain.indices.chunks(3) {
            let p: Vec<[f32; 3]> = tri.iter().map(|&i| g.terrain.vertices[i as usize]).collect();
            let cross_y = (p[1][2] - p[0][2]) * (p[2][0] - p[0][0]) - (p[1][0] - p[0][0]) * (p[2][2] - p[0][2]);
            assert!(cross_y < 0.0, "triangle {tri:?} is back-facing for Godot");
        }
    }

    #[test]
    fn terrain_normals_follow_the_slope() {
        // Height rises 0.5 m per meter eastward: interior normals tilt west (-x).
        let t = tile(0, 0, grid(5, |u, _| (u * 20.0 * 0.5) as f32), Vec::new());
        let g = tile_geometry(&t, 20.0);
        let n = g.terrain.normals[2 * 5 + 2];
        let len = (0.5f32 * 0.5 + 1.0).sqrt();
        assert!((n[0] + 0.5 / len).abs() < 1e-4, "{n:?}");
        assert!((n[1] - 1.0 / len).abs() < 1e-4, "{n:?}");
        assert!(n[2].abs() < 1e-4, "{n:?}");
    }

    #[test]
    fn terrain_world_axes_match_godot() {
        // Local (x = 10, y = 20) is east 10 m and north 20 m: Godot (10, h, -20).
        let t = tile(0, 1, grid(3, |_, _| 4.0), Vec::new());
        let g = tile_geometry(&t, 20.0);
        // Vertex row 0 is at y = 20 (tile y = 1), column 1 at x = 10.
        assert_eq!(g.terrain.vertices[1], [10.0, 4.0, -20.0]);
    }

    #[test]
    fn road_becomes_front_up_ribbon_on_terrain() {
        let road = Polyline { z: Vec::new(),
            highway: "residential".into(),
            points: vec![[0.0, 10.0], [20.0, 10.0]],
        };
        let t = tile(0, 0, grid(3, |_, _| 50.0), vec![road]);
        let g = tile_geometry(&t, 20.0);
        // Both ends lie on tile borders, so the ribbon runs BORDER_OVERLAP_M past each.
        let xs: Vec<f32> = g.roads.vertices.iter().map(|v| v[0]).collect();
        let (lo, hi) = (BORDER_OVERLAP_M as f32, 20.0 + BORDER_OVERLAP_M as f32);
        assert!(xs.iter().any(|x| (x + lo).abs() < 1e-4));
        assert!(xs.iter().any(|x| (x - hi).abs() < 1e-4));
        assert_eq!(g.roads.indices.len() / 3, (g.roads.vertices.len() / 3 - 1) * 4);
        for v in &g.roads.vertices {
            assert!((v[1] - (50.0 + ROAD_LIFT_M)).abs() < 1e-4);
        }
        // Ribbon is 6 m wide: corners at z = -(10 +- 3).
        let zs: Vec<f32> = g.roads.vertices.iter().map(|v| v[2]).collect();
        assert!(zs.iter().any(|z| (z + 13.0).abs() < 1e-4));
        assert!(zs.iter().any(|z| (z + 7.0).abs() < 1e-4));
        for tri in g.roads.indices.chunks(3) {
            let p: Vec<[f32; 3]> = tri.iter().map(|&i| g.roads.vertices[i as usize]).collect();
            let cross_y = (p[1][2] - p[0][2]) * (p[2][0] - p[0][0]) - (p[1][0] - p[0][0]) * (p[2][2] - p[0][2]);
            assert!(cross_y < 0.0);
        }
    }

    /// A 12 m primary road ends on the centerline of a 6 m residential road, at 45 degrees.
    /// Its cut is perpendicular to it, so the corner on the near side pokes out of the
    /// crossed road and leaves a wedge uncovered between the cut and the crossed road's
    /// edge. Points of A's band beyond its end, outside the crossed road on A's side, must
    /// be covered.
    #[test]
    fn road_end_on_crossed_road_leaves_no_wedge() {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let end = [125.0, 125.0];
        let a = Polyline { z: Vec::new(), highway: "primary".into(), points: vec![[end[0] - 80.0 * s, end[1] - 80.0 * s], end] };
        let b = Polyline { z: Vec::new(), highway: "residential".into(), points: vec![[0.0, 125.0], [250.0, 125.0]] };
        let t = tile(0, 0, grid(26, |_, _| 50.0), vec![a, b]);
        let g = tile_geometry(&t, 250.0);
        let (half_a, half_b) = (6.0, 3.0);
        let mut missing = Vec::new();
        for ia in 0..=24 {
            for ib in 0..=24 {
                // Along the road (a from the end, up to half_b past it) and across it.
                let along = half_b * ia as f64 / 24.0;
                let across = (half_a - 0.2) * (2.0 * ib as f64 / 24.0 - 1.0);
                let x = end[0] + along * s - across * s;
                let y = end[1] + along * s + across * s;
                // Near side of the crossed road only (its far side may be overshot).
                if y > end[1] - half_b {
                    continue;
                }
                if ribbon_height(&g.roads, x, y).is_none() {
                    missing.push((x, y));
                }
            }
        }
        assert!(missing.is_empty(), "{} uncovered points, e.g. {:?}", missing.len(), missing.first());
    }

    #[test]
    fn height_at_finds_the_tile_and_none_outside() {
        let a = tile(0, 0, grid(3, |_, _| 10.0), Vec::new());
        let b = tile(-1, 0, grid(3, |_, _| 20.0), Vec::new());
        let map = Map::new(meta(true), vec![b, a]);
        assert_eq!(map.height_at(0.0, 0.0), Some(10.0));
        assert_eq!(map.height_at(-0.5, 5.0), Some(20.0));
        assert_eq!(map.height_at(25.0, 0.0), None);
        assert_eq!(map.height_at(0.0, -1.0), None);
    }

    #[test]
    fn load_map_reads_meta_and_tiles() {
        let root = temp_cache_root("ok");
        let dir = root.join("t");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("meta.json"), serde_json::to_string(&meta(true)).unwrap()).unwrap();
        write_tile(&root, "t", &tile(1, 0, grid(3, |_, _| 2.0), Vec::new())).unwrap();
        write_tile(&root, "t", &tile(0, 0, grid(3, |_, _| 1.0), Vec::new())).unwrap();
        fs::write(dir.join("tiles").join("notes.txt"), "ignored").unwrap();

        let map = load_map(&root, "t").unwrap();
        assert_eq!(map.tiles.iter().map(|t| (t.x, t.y)).collect::<Vec<_>>(), vec![(0, 0), (1, 0)]);
        assert_eq!(map.height_at(0.0, 0.0), Some(1.0));
        assert_eq!(map.height_at(30.0, 0.0), Some(2.0));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn load_map_rejects_incomplete_map() {
        let root = temp_cache_root("incomplete");
        let dir = root.join("t");
        fs::create_dir_all(dir.join("tiles")).unwrap();
        fs::write(dir.join("meta.json"), serde_json::to_string(&meta(false)).unwrap()).unwrap();
        assert!(load_map(&root, "t").is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn neighborhood_geometry_matches_whole_map() {
        let root = temp_cache_root("nbhd");
        let dir = root.join("t");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("meta.json"), serde_json::to_string(&meta(true)).unwrap()).unwrap();
        let road = |points: Vec<[f64; 2]>| Polyline { z: Vec::new(),
            highway: "residential".into(),
            points,
        };
        // A road crosses the border between (0, 0) and (1, 0); the others are sloped.
        write_tile(&root, "t", &tile(0, 0, grid(3, |u, _| (3.0 * u) as f32), vec![road(vec![[15.0, 10.0], [25.0, 10.0]])])).unwrap();
        write_tile(&root, "t", &tile(1, 0, grid(3, |u, v| (20.0 + 4.0 * v + u) as f32), vec![road(vec![[22.0, 5.0], [33.0, 15.0]])])).unwrap();
        write_tile(&root, "t", &tile(0, 1, grid(3, |_, v| (5.0 * v) as f32), Vec::new())).unwrap();

        let whole = load_map(&root, "t").unwrap();
        let index = open_map(&root, "t").unwrap();
        for tile in &whole.tiles {
            let from_index = index.tile_geometry(tile.x, tile.y).unwrap();
            assert_eq!(from_index, whole.tile_geometry(tile), "tile {} {}", tile.x, tile.y);
        }
        fs::remove_dir_all(&root).unwrap();
    }

    /// Diagnostic on the real baked map (run with `--ignored --nocapture`): along every tile
    /// edge, compares whether the road ribbon covers the point on each side, and its height.
    #[test]
    #[ignore]
    fn real_map_road_seams() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache");
        let t0 = std::time::Instant::now();
        let map = load_map(&root, "casa").expect("baked map 'casa'");
        println!("load_map_ms={}", t0.elapsed().as_millis());
        let m = map.meta.tile_size_m;
        let t0 = std::time::Instant::now();
        let geo: Vec<_> = map.tiles.iter().map(|t| (t.x, t.y, tile_geometry(t, m))).collect();
        println!("tile_geometry_ms={} tiles={}", t0.elapsed().as_millis(), geo.len());
        let road_tris: usize = geo.iter().map(|g| g.2.roads.indices.len() / 3).sum();
        println!("road_triangles={road_tris}");
        let find = |x: i32, y: i32| geo.iter().find(|g| g.0 == x && g.1 == y).map(|g| &g.2);
        let roads: Vec<(f64, &Polyline)> = map.tiles.iter().flat_map(|t| t.roads.iter()).map(|r| (road_width_m(&r.highway) / 2.0, r)).collect();
        let truth = |x: f64, y: f64| {
            roads.iter().any(|(half, r)| {
                r.points.windows(2).any(|w| {
                    let (dx, dy) = (w[1][0] - w[0][0], w[1][1] - w[0][1]);
                    let l2 = dx * dx + dy * dy;
                    let t = if l2 > 0.0 { (((x - w[0][0]) * dx + (y - w[0][1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
                    ((x - w[0][0] - t * dx).hypot(y - w[0][1] - t * dy)) < *half - 0.1
                })
            })
        };
        let (mut gaps, mut height_diff, mut checked) = (0, 0, 0);
        for (tx, ty, a) in &geo {
            for (dx, dy) in [(1, 0), (0, 1)] {
                let Some(b) = find(tx + dx, ty + dy) else { continue };
                let n = (m / 0.25) as usize;
                for k in 0..=n {
                    let s = k as f64 * 0.25;
                    // Near a tile corner the diagonal tile may draw the road; not checked here.
                    let corner = s < 4.0 || s > m - 4.0;
                    let (ex, ey) = if dx == 1 { ((*tx + 1) as f64 * m, *ty as f64 * m + s) } else { (*tx as f64 * m + s, (*ty + 1) as f64 * m) };
                    let (ax, ay, bx, by) = if dx == 1 { (ex - 1e-4, ey, ex + 1e-4, ey) } else { (ex, ey - 1e-4, ex, ey + 1e-4) };
                    let (ha, hb) = (ribbon_height(&a.roads, ax, ay), ribbon_height(&b.roads, bx, by));
                    checked += 1;
                    let any_tile = |x: f64, y: f64| geo.iter().any(|g| ribbon_height(&g.2.roads, x, y).is_some());
                    let covered = if corner { any_tile(ex - 1e-4, ey - 1e-4) || any_tile(ex + 1e-4, ey + 1e-4) || any_tile(ex - 1e-4, ey + 1e-4) || any_tile(ex + 1e-4, ey - 1e-4) } else { ha.is_some() || hb.is_some() };
                    if truth(ex, ey) && !covered {
                        gaps += 1;
                        if gaps <= 20 {
                            println!("GAP: road covers edge point ({ex:.2}, {ey:.2}) but neither tile draws it");
                        }
                    }
                    if let (Some(u), Some(v)) = (ha, hb) {
                        if (u - v).abs() > 0.02 {
                            height_diff += 1;
                            if height_diff <= 10 {
                                println!("height differs at ({ex:.2}, {ey:.2}): {u:.3} vs {v:.3}");
                            }
                        }
                    }
                }
            }
        }
        println!("checked={checked} gaps={gaps} height_diff={height_diff}");
    }

    #[test]
    fn road_over_a_tile_border_follows_the_neighbor_terrain() {
        // Tile (0,0) is flat at 10 m; tile (1,0) is flat at 20 m. A road along y = 10 ends
        // at x = 20 + 3 (it crosses the border), so its tail lies over the neighbor.
        let road = Polyline { z: Vec::new(), highway: "residential".into(), points: vec![[10.0, 10.0], [23.0, 10.0]] };
        let a = tile(0, 0, grid(3, |_, _| 10.0), vec![road]);
        let b = tile(1, 0, grid(3, |_, _| 20.0), Vec::new());
        let map = Map::new(meta(true), vec![a.clone(), b]);
        let g = map.tile_geometry(&a);
        let h = ribbon_height(&g.roads, 22.0, 10.0).unwrap();
        assert!((h - (20.0 + ROAD_LIFT_M as f64)).abs() < 0.05, "h = {h}");
    }
}
