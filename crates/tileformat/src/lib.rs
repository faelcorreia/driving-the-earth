//! Formato do mapa salvo em disco. Compartilhado entre o bake e o leitor do jogo.
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Conteúdo de `cache/<id>/meta.json`.
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

/// Conteúdo de `cache/<id>/tiles/x_y.json`: um tile da grade do mapa.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tile {
    /// Índice do tile na grade (x, y).
    pub x: i32,
    pub y: i32,
    /// Alturas em grade (DSM já suavizado), linha-major: `values[row * cols + col]`.
    pub height: HeightGrid,
    /// Polilinhas de vias.
    pub roads: Vec<Polyline>,
    /// Polígonos de prédios.
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
    /// Valor da tag `highway` do OSM (ex.: "residential").
    pub highway: String,
    /// Pontos `[x, y]` no sistema definido pelo bake.
    pub points: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Polygon {
    /// Anel externo `[x, y]` no sistema definido pelo bake.
    pub points: Vec<[f64; 2]>,
}

/// Caminho de `cache/<id>/tiles/x_y.json` a partir da raiz do cache (`cache/`).
pub fn tile_path(cache_root: &Path, id: &str, x: i32, y: i32) -> PathBuf {
    cache_root
        .join(id)
        .join("tiles")
        .join(format!("{x}_{y}.json"))
}

/// Grava o tile em `cache/<id>/tiles/x_y.json` (cria as pastas se faltarem).
/// Retorna o caminho gravado.
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

/// Lê um tile de `cache/<id>/tiles/x_y.json`.
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
    fn round_trip_identico() {
        let root = temp_cache_root("rt");
        let tile = sample_tile();
        let path = write_tile(&root, "casa", &tile).unwrap();
        assert_eq!(path, root.join("casa/tiles/-3_7.json"));
        let back = read_tile(&path).unwrap();
        assert_eq!(back, tile);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn caminho_do_tile() {
        let p = tile_path(Path::new("cache"), "casa", 1, -2);
        assert_eq!(p, Path::new("cache/casa/tiles/1_-2.json"));
    }
}
