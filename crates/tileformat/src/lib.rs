//! On-disk map format. Shared between the bake and the game reader.
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Contents of `cache/<id>/meta.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapMeta {
    pub id: String,
    pub name: String,
    pub center_lat: f64,
    pub center_lon: f64,
    pub radius_m: f64,
    pub tile_size_m: f64,
    pub created_unix: u64,
    pub complete: bool,
}

/// Contents of `cache/<id>/tiles/x_y.json`: one tile of the map grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tile {
    /// Tile index in the grid (x, y).
    pub x: i32,
    pub y: i32,
    /// Grid heights (DSM, already smoothed), row-major: `values[row * cols + col]`.
    pub height: HeightGrid,
    /// Road polylines.
    pub roads: Vec<Polyline>,
    /// Building polygons.
    pub buildings: Vec<Polygon>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeightGrid {
    pub cols: u32,
    pub rows: u32,
    pub values: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Polyline {
    /// Value of the OSM `highway` tag (e.g. "residential").
    pub highway: String,
    /// Points `[x, y]` in the system defined by the bake.
    pub points: Vec<[f64; 2]>,
    /// Terrain-following height of each point (same length as `points`). Empty when the
    /// bake gave no profile: the road then follows the terrain under it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub z: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Polygon {
    /// Outer ring `[x, y]` in the system defined by the bake.
    pub points: Vec<[f64; 2]>,
}

/// Path of `cache/<id>/tiles/x_y.json` from the cache root (`cache/`).
pub fn tile_path(cache_root: &Path, id: &str, x: i32, y: i32) -> PathBuf {
    cache_root
        .join(id)
        .join("tiles")
        .join(format!("{x}_{y}.json"))
}

/// Writes the tile to `cache/<id>/tiles/x_y.json` (creates the folders if missing).
/// Returns the path written.
pub fn write_tile(cache_root: &Path, id: &str, tile: &Tile) -> io::Result<PathBuf> {
    let path = tile_path(cache_root, id, tile.x, tile.y);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let file = fs::File::create(&path)?;
    serde_json::to_writer(io::BufWriter::new(file), tile)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(path)
}

/// Reads a tile from `cache/<id>/tiles/x_y.json`.
pub fn read_tile(path: &Path) -> io::Result<Tile> {
    let file = fs::File::open(path)?;
    serde_json::from_reader(io::BufReader::new(file))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tile() -> Tile {
        Tile {
            x: -3,
            y: 7,
            height: HeightGrid {
                cols: 3,
                rows: 2,
                values: vec![812.5, 813.25, 0.1, 799.999_9, -1.0, 800.0],
            },
            roads: vec![Polyline {
                highway: "residential".into(),
                points: vec![[-43.9123456789, -19.9234567891], [-43.9, -19.92]],
                z: Vec::new(),
            }],
            buildings: vec![Polygon {
                points: vec![[0.0, 0.0], [10.5, 0.0], [10.5, 8.25], [0.0, 8.25]],
            }],
        }
    }

    fn temp_cache_root(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("tileformat-{tag}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn round_trip_is_identical() {
        let root = temp_cache_root("rt");
        let tile = sample_tile();
        let path = write_tile(&root, "casa", &tile).unwrap();
        assert_eq!(path, root.join("casa/tiles/-3_7.json"));
        let back = read_tile(&path).unwrap();
        assert_eq!(back, tile);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn tile_path_layout() {
        let p = tile_path(Path::new("cache"), "casa", 1, -2);
        assert_eq!(p, Path::new("cache/casa/tiles/1_-2.json"));
    }
}
