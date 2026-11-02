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
const ROAD_LIFT_M: f32 = 0.3;

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

/// Bilinear height of `tile` at local `(x, y)`. Points outside the tile are clamped to its edge.
/// The grid has `cols` samples per side spread over `tile_m` meters, edges included.
pub fn sample_height(tile: &Tile, tile_m: f64, x: f64, y: f64) -> f32 {
    let grid = &tile.height;
    let n = grid.cols as usize;
    let last = (n - 1) as f64;
    let cell = tile_m / last;
    let u = ((x - tile.x as f64 * tile_m) / cell).clamp(0.0, last);
    let v = ((y - tile.y as f64 * tile_m) / cell).clamp(0.0, last);
    let (j0, i0) = (u.floor() as usize, v.floor() as usize);
    let (j1, i1) = ((j0 + 1).min(n - 1), (i0 + 1).min(n - 1));
    let (fu, fv) = (u - j0 as f64, v - i0 as f64);
    let at = |i: usize, j: usize| grid.values[i * n + j] as f64;
    let top = at(i0, j0) + (at(i0, j1) - at(i0, j0)) * fu;
    let bottom = at(i1, j0) + (at(i1, j1) - at(i1, j0)) * fu;
    (top + (bottom - top) * fv) as f32
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
    for road in &tile.roads {
        let half = road_width_m(&road.highway) / 2.0;
        for seg in road.points.windows(2) {
            let (p, q) = (seg[0], seg[1]);
            let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-9 {
                continue;
            }
            let (nx, ny) = (-dy / len * half, dx / len * half);
            let corners = [
                [p[0] + nx, p[1] + ny],
                [p[0] - nx, p[1] - ny],
                [q[0] - nx, q[1] - ny],
                [q[0] + nx, q[1] + ny],
            ];
            let base = roads.vertices.len() as u32;
            for [cx, cy] in corners {
                let height = sample_height(tile, tile_m, cx, cy) + ROAD_LIFT_M;
                roads.vertices.push(world(cx, cy, height));
                roads.normals.push([0.0, 1.0, 0.0]);
            }
            push_up_facing(&mut roads, base, base + 1, base + 2);
            push_up_facing(&mut roads, base, base + 2, base + 3);
        }
    }

    TileGeometry { terrain, roads }
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
    fn sample_height_is_bilinear() {
        // 3x3 samples over 20 m (cell 10 m): height = x_m + 100 (ramp along x).
        let t = tile(0, 0, grid(3, |u, _| (u * 20.0) as f32 + 100.0), Vec::new());
        assert!((sample_height(&t, 20.0, 0.0, 0.0) - 100.0).abs() < 1e-4);
        assert!((sample_height(&t, 20.0, 5.0, 7.0) - 105.0).abs() < 1e-4);
        assert!((sample_height(&t, 20.0, 20.0, 3.0) - 120.0).abs() < 1e-4);
        // Outside the tile: clamped to the edge.
        assert!((sample_height(&t, 20.0, 50.0, -9.0) - 120.0).abs() < 1e-4);
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
        assert_eq!(g.roads.vertices.len(), 4);
        assert_eq!(g.roads.indices.len(), 6);
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
