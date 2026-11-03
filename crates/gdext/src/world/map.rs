//! Baked-map index and in-memory map: opens `cache/<id>/` and gives tile geometry with the
//! neighbors' terrain. Layer: `world`. May import `geometry`, `tileformat` and `tilebake`.
//! Imports no Godot types.
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tileformat::{read_tile, MapMeta, Tile};

use crate::geometry::{sample_height, tile_geometry_with, TileGeometry};

/// A baked map: its metadata and every tile found in `cache/<id>/tiles/`.
pub struct Map {
    pub meta: MapMeta,
    /// Sorted by `(x, y)`.
    pub tiles: Vec<Tile>,
    index: HashMap<(i32, i32), usize>,
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

    /// Geometry of `tile` with its neighbors' roads and grid: the surface is the same one the
    /// neighbor's tile sees at a shared border.
    pub fn tile_geometry(&self, tile: &Tile) -> TileGeometry {
        let size = self.meta.tile_size_m;
        let around: Vec<&Tile> = self
            .tiles
            .iter()
            .filter(|t| (t.x - tile.x).abs() <= 1 && (t.y - tile.y).abs() <= 1 && (t.x, t.y) != (tile.x, tile.y))
            .collect();
        tile_geometry_with(tile, size, &|x, y| self.height_at(x, y).unwrap_or_else(|| sample_height(tile, size, x, y)), &around)
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
