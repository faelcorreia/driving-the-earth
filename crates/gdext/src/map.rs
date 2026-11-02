//! Map logic without Godot types: loads a baked map from `cache/<id>/` and turns
//! each tile into plain triangle meshes, so it can be unit-tested with `cargo test`.
//!
//! Coordinates: the tile plane is local meters, `x` east and `y` north, with the
//! home (map center) at the origin. Godot world space is `x` east, `y` up and
//! `z = -north`, so the car's forward (-Z) points north.
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

use tileformat::{read_tile, MapMeta, Tile};

/// Height of the road ribbons above the terrain, in meters (avoids z-fighting).
const ROAD_LIFT_M: f32 = 0.1;

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
}

fn invalid_data<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

/// Loads `cache_root/<id>/meta.json` and every `tiles/*.json`.
/// Fails when the map is not complete (tilebake has not finished) or a tile is malformed.
pub fn load_map(cache_root: &Path, id: &str) -> io::Result<Map> {
    let dir = cache_root.join(id);
    let meta: MapMeta = serde_json::from_str(&fs::read_to_string(dir.join("meta.json"))?).map_err(invalid_data)?;
    if !meta.complete {
        return Err(invalid_data(format!("map '{id}' is not complete, run tilebake first")));
    }
    let mut tiles = Vec::new();
    for entry in fs::read_dir(dir.join("tiles"))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            let tile = read_tile(&path)?;
            let n = tile.height.cols as usize;
            if n < 2 || tile.height.rows as usize != n || tile.height.values.len() != n * n {
                return Err(invalid_data(format!("tile {} {} has a bad height grid", tile.x, tile.y)));
            }
            tiles.push(tile);
        }
    }
    tiles.sort_by_key(|t| (t.x, t.y));
    Ok(Map::new(meta, tiles))
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

/// Builds the terrain grid and the road ribbons of one tile.
pub fn tile_geometry(tile: &Tile, tile_m: f64) -> TileGeometry {
    let n = tile.height.cols as usize;
    let cell = tile_m / (n - 1) as f64;
    let (x0, y0) = (tile.x as f64 * tile_m, tile.y as f64 * tile_m);
    let h = |i: usize, j: usize| tile.height.values[i * n + j];

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
    for road in &tile.roads {
        let half = road_width_m(&road.highway) / 2.0;
        push_road(&mut roads, tile, tile_m, &road.points, half, step);
    }

    TileGeometry { terrain, roads }
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
fn push_road(mesh: &mut MeshData, tile: &Tile, tile_m: f64, points: &[[f64; 2]], half: f64, step: f64) {
    // Drop repeated points: they have no direction.
    let mut pts: Vec<[f64; 2]> = Vec::with_capacity(points.len());
    for &p in points {
        match pts.last() {
            Some(&q) if dist(p, q) <= 1e-9 => {}
            _ => pts.push(p),
        }
    }
    if pts.len() < 2 {
        return;
    }
    let normals: Vec<[f64; 2]> = pts.windows(2).map(|w| unit_normal(w[0], w[1])).collect();

    // Offset from the center to the left edge, one per station, with its position.
    let mut stations: Vec<([f64; 2], [f64; 2])> = Vec::new();
    stations.push((pts[0], scale(normals[0], half)));
    for k in 0..pts.len() - 1 {
        let (p, q) = (pts[k], pts[k + 1]);
        let pieces = (dist(p, q) / step).ceil().max(1.0) as usize;
        for s in 1..pieces {
            let t = s as f64 / pieces as f64;
            let at = [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t];
            stations.push((at, scale(normals[k], half)));
        }
        let offset = match normals.get(k + 1) {
            Some(&next) => miter(normals[k], next, half),
            None => scale(normals[k], half),
        };
        stations.push((q, offset));
    }

    let base = mesh.vertices.len() as u32;
    for &(c, o) in &stations {
        for [x, y] in [[c[0] + o[0], c[1] + o[1]], c, [c[0] - o[0], c[1] - o[1]]] {
            let height = sample_height(tile, tile_m, x, y) + ROAD_LIFT_M;
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
        let road = Polyline {
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
        let piece = |x0: f64, x1: f64| Polyline {
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
        let road = Polyline {
            highway: "residential".into(),
            points: vec![[0.0, 10.0], [20.0, 10.0]],
        };
        let t = tile(0, 0, grid(3, |_, _| 50.0), vec![road]);
        let g = tile_geometry(&t, 20.0);
        // 20 m at a station every 2.5 m: 9 stations x 3 vertices, 8 x 4 triangles.
        assert_eq!(g.roads.vertices.len(), 27);
        assert_eq!(g.roads.indices.len(), 8 * 4 * 3);
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
}
