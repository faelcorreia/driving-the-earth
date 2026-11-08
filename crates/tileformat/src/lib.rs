//! On-disk map format. Shared between the bake and the game reader.
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub mod far_lights;
pub mod roads;
pub mod terrain;
pub mod units;

/// Ground classes of `Tile::ground` (one byte per cell of `units::GROUND_CELL_M`).
pub const GROUND_GRASS: u8 = 0;
pub const GROUND_CONCRETE: u8 = 1;
pub const GROUND_SIDEWALK: u8 = 2;
pub const GROUND_PARK: u8 = 3;
pub const GROUND_WATER: u8 = 4;

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
    /// Building polygons (real from OSM, and generated lots with `Polygon::generated`).
    pub buildings: Vec<Polygon>,
    /// Ground class per cell, row-major, `units::GROUND_N` x `units::GROUND_N` cells of
    /// `units::GROUND_CELL_M`. Empty when the bake gave no ground map (old caches).
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "ground_rle")]
    pub ground: Vec<u8>,
    /// Heightfield at `units::TERRAIN_FINE_CELL_M` (2.5 m), stored as a base64 string of its binary
    /// blob (see `encode_fine`). `None` in tiles baked before it existed: readers then use `height`.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "fine_codec")]
    pub fine_height: Option<Heightfield>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeightGrid {
    pub cols: u32,
    pub rows: u32,
    pub values: Vec<f32>,
}

/// A fine heightfield of one tile: `cols` x `rows` samples, edges included, row-major. Sample `k`
/// is `base_cm + cm[k]` centimeters. The base keeps the absolute altitude (about 900 m in Belo
/// Horizonte) inside the i16 range: `cm` covers `base_cm` plus or minus 327 m.
#[derive(Debug, Clone, PartialEq)]
pub struct Heightfield {
    pub cols: u32,
    pub rows: u32,
    pub base_cm: i32,
    pub cm: Vec<i16>,
}

impl Heightfield {
    /// Quantizes heights in meters (row-major, `cols` x `rows`) to centimeters, rounded.
    pub fn from_meters(cols: u32, rows: u32, values: &[f32]) -> Result<Heightfield, String> {
        if values.len() != (cols as usize) * (rows as usize) || values.is_empty() {
            return Err(format!(
                "heightfield {cols}x{rows} needs {} values, got {}",
                cols as usize * rows as usize,
                values.len()
            ));
        }
        if values.iter().any(|v| !v.is_finite()) {
            return Err("heightfield has a non-finite height".into());
        }
        let min = values.iter().fold(f32::INFINITY, |a, &b| a.min(b)) as f64;
        let max = values.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b)) as f64;
        let base_cm = (((min + max) / 2.0) * 100.0).round() as i32;
        let mut cm = Vec::with_capacity(values.len());
        for &v in values {
            let rel = (v as f64 * 100.0).round() - base_cm as f64;
            if rel < i16::MIN as f64 || rel > i16::MAX as f64 {
                return Err(format!(
                    "heightfield relief {:.1} m does not fit in i16 cm around its base",
                    (max - min)
                ));
            }
            cm.push(rel as i16);
        }
        Ok(Heightfield {
            cols,
            rows,
            base_cm,
            cm,
        })
    }

    /// Height of sample `k` in meters.
    pub fn meters(&self, k: usize) -> f32 {
        ((self.base_cm as i64 + self.cm[k] as i64) as f64 / 100.0) as f32
    }
}

/// Version of the binary heightfield blob. Readers reject any other version.
pub const FINE_VERSION: u8 = 1;
/// Bytes before the samples: version u8, cols u16, rows u16, base_cm i32.
const FINE_HEADER_LEN: usize = 9;

/// Binary blob of a heightfield, little endian: `version u8 | cols u16 | rows u16 | base_cm i32 |
/// cm i16 x (cols * rows)`, row-major.
pub fn encode_fine(f: &Heightfield) -> Vec<u8> {
    debug_assert!(f.cols <= u16::MAX as u32 && f.rows <= u16::MAX as u32);
    let mut out = Vec::with_capacity(FINE_HEADER_LEN + 2 * f.cm.len());
    out.push(FINE_VERSION);
    out.extend((f.cols as u16).to_le_bytes());
    out.extend((f.rows as u16).to_le_bytes());
    out.extend(f.base_cm.to_le_bytes());
    for v in &f.cm {
        out.extend(v.to_le_bytes());
    }
    out
}

/// Reads a blob written by `encode_fine`.
pub fn decode_fine(bytes: &[u8]) -> Result<Heightfield, String> {
    if bytes.len() < FINE_HEADER_LEN {
        return Err(format!("heightfield blob too short: {} bytes", bytes.len()));
    }
    if bytes[0] != FINE_VERSION {
        return Err(format!("unknown heightfield version {}", bytes[0]));
    }
    let cols = u16::from_le_bytes([bytes[1], bytes[2]]) as u32;
    let rows = u16::from_le_bytes([bytes[3], bytes[4]]) as u32;
    let base_cm = i32::from_le_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]);
    let count = cols as usize * rows as usize;
    if bytes.len() != FINE_HEADER_LEN + 2 * count {
        return Err(format!(
            "heightfield blob is {} bytes, expected {} for {cols}x{rows}",
            bytes.len(),
            FINE_HEADER_LEN + 2 * count
        ));
    }
    let cm = bytes[FINE_HEADER_LEN..]
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    Ok(Heightfield {
        cols,
        rows,
        base_cm,
        cm,
    })
}

/// Standard base64 (RFC 4648, padded), so the binary heightfield can sit in the JSON tile.
mod base64 {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b1 = *chunk.get(1).unwrap_or(&0);
            let b2 = *chunk.get(2).unwrap_or(&0);
            let n = (chunk[0] as u32) << 16 | (b1 as u32) << 8 | b2 as u32;
            for k in 0..4 {
                if k <= chunk.len() {
                    out.push(ALPHABET[(n >> (18 - 6 * k)) as usize & 63] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    pub fn decode(text: &str) -> Result<Vec<u8>, String> {
        if text.len() % 4 != 0 {
            return Err("base64 length is not a multiple of 4".into());
        }
        let body = text.trim_end_matches('=');
        if text.len() - body.len() > 2 {
            return Err("too much base64 padding".into());
        }
        let mut out = Vec::with_capacity(body.len() * 3 / 4);
        let (mut acc, mut bits) = (0u32, 0u32);
        for c in body.bytes() {
            let v = match c {
                b'A'..=b'Z' => c - b'A',
                b'a'..=b'z' => c - b'a' + 26,
                b'0'..=b'9' => c - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => return Err(format!("invalid base64 byte {c}")),
            };
            acc = (acc << 6) | v as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
        Ok(out)
    }
}

/// The heightfield travels in the tile JSON as a base64 string of its binary blob.
mod fine_codec {
    use super::{base64, decode_fine, encode_fine, Heightfield};
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(fine: &Option<Heightfield>, s: S) -> Result<S::Ok, S::Error> {
        match fine {
            Some(f) => s.serialize_str(&base64::encode(&encode_fine(f))),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Heightfield>, D::Error> {
        let Some(text) = Option::<String>::deserialize(d)? else {
            return Ok(None);
        };
        let bytes = base64::decode(&text).map_err(D::Error::custom)?;
        decode_fine(&bytes).map(Some).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Polyline {
    /// Value of the OSM `highway` tag (e.g. "residential").
    pub highway: String,
    /// Value of the OSM `name` tag; empty when the way has none. Used by the 2D map labels.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Points `[x, y]` in the system defined by the bake.
    pub points: Vec<[f64; 2]>,
    /// Terrain-following height of each point (same length as `points`). Empty when the
    /// bake gave no profile: the road then follows the terrain under it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub z: Vec<f32>,
    /// Height of the left edge of the road at each point (`z` minus half the width times the
    /// transverse slope of the terrain). Same length as `z`; empty when there is no profile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zl: Vec<f32>,
    /// Height of the right edge at each point (`z` plus half the width times the transverse slope).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zr: Vec<f32>,
    /// OSM `lit` tag: `Some(true)` for "yes", `Some(false)` for "no"; `None` when the way has no
    /// `lit` tag or another value, and in tiles baked before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lit: Option<bool>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Polygon {
    /// Outer ring `[x, y]` in the system defined by the bake.
    pub points: Vec<[f64; 2]>,
    /// True for a building generated by the bake (fills a free lot); false for a real OSM building.
    #[serde(default, skip_serializing_if = "is_false")]
    pub generated: bool,
    /// Number of floors; 0 when unknown.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub levels: u8,
    /// Index of the Brazilian palette: 0 orange brick/tile, 1 ice white, 2 light gray, 3 dark gray.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub color_idx: u8,
    /// Small deterministic variation of the color, from the building seed.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tint: u8,
}

impl Polygon {
    /// A real building from OSM: no generated data.
    pub fn real(points: Vec<[f64; 2]>) -> Polygon {
        Polygon {
            points,
            generated: false,
            levels: 0,
            color_idx: 0,
            tint: 0,
        }
    }
}

/// The ground map is stored run-length encoded: flat `[value, run, value, run, ...]` (most of a tile is
/// one or two classes). In memory it is the plain `Vec<u8>` of one byte per cell.
mod ground_rle {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(ground: &[u8], s: S) -> Result<S::Ok, S::Error> {
        let mut flat: Vec<u32> = Vec::new();
        for &v in ground {
            match flat.len() {
                n if n >= 2 && flat[n - 2] == v as u32 => flat[n - 1] += 1,
                _ => flat.extend([v as u32, 1]),
            }
        }
        s.collect_seq(flat)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let flat: Vec<u32> = Vec::deserialize(d)?;
        if flat.len() % 2 != 0 {
            return Err(serde::de::Error::custom(
                "ground runs must come in (value, count) pairs",
            ));
        }
        let mut out = Vec::new();
        for pair in flat.chunks(2) {
            out.extend(std::iter::repeat(pair[0] as u8).take(pair[1] as usize));
        }
        Ok(out)
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(v: &u8) -> bool {
    *v == 0
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

/// Reads only the road polylines of a tile file (the heights and buildings are skipped).
pub fn read_tile_roads(path: &Path) -> io::Result<Vec<Polyline>> {
    #[derive(Deserialize)]
    struct RoadsOnly {
        roads: Vec<Polyline>,
    }
    let file = fs::File::open(path)?;
    let tile: RoadsOnly = serde_json::from_reader(io::BufReader::new(file))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(tile.roads)
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
                name: "Rua Exemplo".into(),
                points: vec![[-43.9123456789, -19.9234567891], [-43.9, -19.92]],
                z: Vec::new(),
                zl: Vec::new(),
                zr: Vec::new(),
                lit: None,
            }],
            buildings: vec![Polygon {
                points: vec![[0.0, 0.0], [10.5, 0.0], [10.5, 8.25], [0.0, 8.25]],
                generated: true,
                levels: 2,
                color_idx: 1,
                tint: 7,
            }],
            ground: vec![GROUND_CONCRETE; units::GROUND_N * units::GROUND_N],
            fine_height: None,
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
    fn old_tile_without_new_fields_reads_with_defaults() {
        let json = r#"{"x":0,"y":0,"height":{"cols":1,"rows":1,"values":[1.0]},
            "roads":[],"buildings":[{"points":[[0.0,0.0],[1.0,0.0],[1.0,1.0]]}]}"#;
        let tile: Tile = serde_json::from_str(json).unwrap();
        let b = &tile.buildings[0];
        assert!(!b.generated && b.levels == 0 && b.color_idx == 0 && b.tint == 0);
        assert!(tile.ground.is_empty());
        let text = serde_json::to_string(&tile).unwrap();
        assert!(!text.contains("ground") && !text.contains("generated"));
    }

    #[test]
    fn road_name_round_trips_and_is_omitted_when_empty() {
        let mut tile = sample_tile();
        let text = serde_json::to_string(&tile).unwrap();
        assert!(text.contains("\"name\":\"Rua Exemplo\""), "{text}");
        let back: Tile = serde_json::from_str(&text).unwrap();
        assert_eq!(back.roads[0].name, "Rua Exemplo");
        tile.roads[0].name.clear();
        let text = serde_json::to_string(&tile).unwrap();
        assert!(!text.contains("name"), "{text}");
        // Tiles baked before the field existed read with an empty name.
        let old: Tile = serde_json::from_str(
            r#"{"x":0,"y":0,"height":{"cols":1,"rows":1,"values":[1.0]},
            "roads":[{"highway":"primary","points":[[0.0,0.0],[1.0,0.0]]}],"buildings":[]}"#,
        )
        .unwrap();
        assert_eq!(old.roads[0].name, "");
    }

    #[test]
    fn lit_round_trips_and_is_omitted_when_absent() {
        let mut tile = sample_tile();
        tile.roads[0].lit = Some(true);
        let text = serde_json::to_string(&tile).unwrap();
        assert!(text.contains("\"lit\":true"), "{text}");
        let back: Tile = serde_json::from_str(&text).unwrap();
        assert_eq!(back.roads[0].lit, Some(true));
        tile.roads[0].lit = Some(false);
        let text = serde_json::to_string(&tile).unwrap();
        assert!(text.contains("\"lit\":false"), "{text}");
        let back: Tile = serde_json::from_str(&text).unwrap();
        assert_eq!(back.roads[0].lit, Some(false));
        tile.roads[0].lit = None;
        let text = serde_json::to_string(&tile).unwrap();
        assert!(!text.contains("lit"), "{text}");
        // Tiles baked before the field existed read with no `lit`.
        let old: Tile = serde_json::from_str(
            r#"{"x":0,"y":0,"height":{"cols":1,"rows":1,"values":[1.0]},
            "roads":[{"highway":"primary","points":[[0.0,0.0],[1.0,0.0]]}],"buildings":[]}"#,
        )
        .unwrap();
        assert_eq!(old.roads[0].lit, None);
    }

    #[test]
    fn read_tile_roads_skips_the_rest_of_the_tile() {
        let root = temp_cache_root("roads");
        let tile = sample_tile();
        let path = write_tile(&root, "casa", &tile).unwrap();
        let roads = read_tile_roads(&path).unwrap();
        assert_eq!(roads, tile.roads);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn tile_path_layout() {
        let p = tile_path(Path::new("cache"), "casa", 1, -2);
        assert_eq!(p, Path::new("cache/casa/tiles/1_-2.json"));
    }
}

#[cfg(test)]
mod ground_rle_tests {
    use super::*;

    #[test]
    fn ground_round_trips_and_is_run_length_encoded() {
        let mut ground = vec![GROUND_GRASS; units::GROUND_N * units::GROUND_N];
        ground[7] = GROUND_CONCRETE;
        ground[8] = GROUND_CONCRETE;
        ground[2499] = GROUND_WATER;
        let tile = Tile {
            x: 1,
            y: -2,
            height: HeightGrid {
                cols: 1,
                rows: 1,
                values: vec![0.0],
            },
            roads: vec![],
            buildings: vec![],
            ground: ground.clone(),
            fine_height: None,
        };
        let json = serde_json::to_string(&tile).unwrap();
        // Runs: grass 7, concrete 2, grass 2490, water 1 -> 8 numbers, far below 2500 cells.
        assert!(json.contains("[0,7,1,2,0,2490,4,1]"), "{json}");
        let back: Tile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.ground, ground);
    }
}

#[cfg(test)]
mod fine_height_tests {
    use super::*;

    fn tile_with(fine: Option<Heightfield>) -> Tile {
        Tile {
            x: 0,
            y: 0,
            height: HeightGrid {
                cols: 1,
                rows: 1,
                values: vec![900.0],
            },
            roads: vec![],
            buildings: vec![],
            ground: Vec::new(),
            fine_height: fine,
        }
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        let cases = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, coded) in cases {
            assert_eq!(base64::encode(plain.as_bytes()), coded);
            assert_eq!(base64::decode(coded).unwrap(), plain.as_bytes());
        }
        assert!(base64::decode("Zm9").is_err());
        assert!(base64::decode("Zm!v").is_err());
    }

    #[test]
    fn blob_has_version_header_and_little_endian_samples() {
        let f = Heightfield {
            cols: 2,
            rows: 1,
            base_cm: 90_012,
            cm: vec![-5, 0x0102],
        };
        let bytes = encode_fine(&f);
        assert_eq!(bytes.len(), 9 + 4);
        assert_eq!(bytes[0], FINE_VERSION);
        assert_eq!(&bytes[1..3], &2u16.to_le_bytes());
        assert_eq!(&bytes[3..5], &1u16.to_le_bytes());
        assert_eq!(&bytes[5..9], &90_012i32.to_le_bytes());
        assert_eq!(&bytes[9..], &[0xFB, 0xFF, 0x02, 0x01]);
        assert_eq!(decode_fine(&bytes).unwrap(), f);
    }

    #[test]
    fn unknown_version_and_short_blob_are_rejected() {
        let f = Heightfield {
            cols: 1,
            rows: 1,
            base_cm: 0,
            cm: vec![1],
        };
        let mut bytes = encode_fine(&f);
        bytes[0] = FINE_VERSION + 1;
        assert!(decode_fine(&bytes).is_err());
        bytes[0] = FINE_VERSION;
        assert!(decode_fine(&bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn fine_height_round_trips_through_tile_json() {
        let f = Heightfield {
            cols: 2,
            rows: 2,
            base_cm: 90_012,
            cm: vec![-5, 0, 7, i16::MAX],
        };
        let tile = tile_with(Some(f.clone()));
        let text = serde_json::to_string(&tile).unwrap();
        assert!(text.contains("\"fine_height\":\""), "{text}");
        let back: Tile = serde_json::from_str(&text).unwrap();
        assert_eq!(back.fine_height, Some(f));
        assert_eq!(back, tile);
    }

    #[test]
    fn tile_without_fine_height_reads_as_none_and_writes_no_key() {
        let json = r#"{"x":0,"y":0,"height":{"cols":1,"rows":1,"values":[1.0]},
            "roads":[],"buildings":[]}"#;
        let tile: Tile = serde_json::from_str(json).unwrap();
        assert!(tile.fine_height.is_none());
        let text = serde_json::to_string(&tile).unwrap();
        assert!(!text.contains("fine_height"), "{text}");
    }

    #[test]
    fn from_meters_rounds_to_cm_and_keeps_absolute_height() {
        let values = [900.004f32, 900.006, 912.5, 927.0];
        let f = Heightfield::from_meters(2, 2, &values).unwrap();
        for (k, &v) in values.iter().enumerate() {
            assert!((f.meters(k) - v).abs() <= 0.005 + 1e-4, "sample {k}: {}", f.meters(k));
        }
        assert_eq!(f.base_cm, 91_350);
    }

    #[test]
    fn from_meters_rejects_relief_beyond_i16_and_bad_input() {
        assert!(Heightfield::from_meters(2, 1, &[0.0, 700.0]).is_err());
        assert!(Heightfield::from_meters(2, 1, &[0.0, f32::NAN]).is_err());
        assert!(Heightfield::from_meters(2, 2, &[0.0, 1.0, 2.0]).is_err());
    }
}
