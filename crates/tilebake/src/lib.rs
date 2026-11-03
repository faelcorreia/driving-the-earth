//! OSM + DEM -> tiles pipeline. Called by the GDExtension on its own thread.
use serde_json::Value;
use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tileformat::MapMeta;

pub mod clip;
pub mod dem;
pub mod far_lights;
pub mod lots;
pub mod osm;
pub mod osm_areas;
pub mod profile;
pub mod region;
pub mod shp;
pub mod tiles;
pub mod utm;

const USER_AGENT: &str = "driving-the-earth/0.1 (tilebake; projeto pessoal)";
const NOMINATIM_URL: &str = "https://nominatim.openstreetmap.org/search";
const OVERPASS_URL: &str = "https://overpass-api.de/api/interpreter";
/// Environment variable with the collection radius, in meters (read when `meta.json` is created).
const RADIUS_ENV: &str = "TILEBAKE_RADIUS_M";

/// Collection radius around the home, in meters, when `TILEBAKE_RADIUS_M` is not set.
pub const DEFAULT_RADIUS_M: f64 = 10000.0;
/// Side of a grid tile, in meters.
pub const DEFAULT_TILE_M: f64 = tileformat::units::TILE_M;

/// Result of `download_osm`: metadata and raw counts.
#[derive(Debug)]
pub struct Summary {
    pub meta: MapMeta,
    /// Ways with a `highway` tag (all blocks, each way counted once).
    pub roads: usize,
    /// Ways with a `building` tag (all blocks, each way counted once).
    pub buildings: usize,
    /// Geocoding came from an existing `meta.json`.
    pub geocoding_cached: bool,
    /// No block was downloaded in this run: all of them were already on disk.
    pub osm_cached: bool,
    /// Blocks that cover the baked tiles.
    pub blocks: usize,
    /// Blocks downloaded in this run.
    pub blocks_downloaded: usize,
    /// Size of `raw/osm/` after the run, in bytes.
    pub osm_bytes: u64,
}

fn other_err<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}

/// Geocodes the address (Nominatim) and returns (lat, lon, raw response).
fn geocode(address: &str) -> io::Result<(f64, f64, String)> {
    let text = ureq::get(NOMINATIM_URL)
        .set("User-Agent", USER_AGENT)
        .query("q", address)
        .query("format", "json")
        .query("limit", "1")
        .call()
        .map_err(other_err)?
        .into_string()?;
    let v: Value = serde_json::from_str(&text).map_err(other_err)?;
    let first = v
        .get(0)
        .ok_or_else(|| other_err("Nominatim found no match for the address"))?;
    let lat: f64 = first["lat"]
        .as_str()
        .ok_or_else(|| other_err("Nominatim response has no 'lat'"))?
        .parse()
        .map_err(other_err)?;
    let lon: f64 = first["lon"]
        .as_str()
        .ok_or_else(|| other_err("Nominatim response has no 'lon'"))?
        .parse()
        .map_err(other_err)?;
    Ok((lat, lon, text))
}

/// Radius from `TILEBAKE_RADIUS_M`, if it is set.
fn radius_from_env() -> io::Result<Option<f64>> {
    match std::env::var(RADIUS_ENV) {
        Ok(text) => {
            let r: f64 = text
                .trim()
                .parse()
                .map_err(|_| other_err(format!("{RADIUS_ENV} is not a number: {text}")))?;
            if r > 0.0 && r.is_finite() {
                Ok(Some(r))
            } else {
                Err(other_err(format!("{RADIUS_ENV} must be a positive number of meters")))
            }
        }
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(e) => Err(other_err(format!("{RADIUS_ENV}: {e}"))),
    }
}

/// Total size in bytes of the files under `path` (0 if it does not exist).
pub fn dir_bytes(path: &Path) -> io::Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        total += if meta.is_dir() {
            dir_bytes(&entry.path())?
        } else {
            meta.len()
        };
    }
    Ok(total)
}

/// Downloads the OSM data around the address and writes it to `cache_root/<id>/raw/osm/`,
/// one file per block of `osm::BLOCK_M` meters (see `osm`).
///
/// Reuses the cache: if `meta.json` exists, it does not geocode again and keeps its radius
/// (a different `TILEBAKE_RADIUS_M` is an error); blocks already on disk are not queried again,
/// so an interrupted run continues where it stopped.
pub fn download_osm(cache_root: &Path, id: &str, name: &str, address: &str) -> io::Result<Summary> {
    let dir = cache_root.join(id);
    let raw = dir.join("raw");
    fs::create_dir_all(&raw)?;
    let meta_path = dir.join("meta.json");
    let nominatim_path = raw.join("nominatim.json");
    let env_radius = radius_from_env()?;

    let (meta, geocoding_cached) = if meta_path.exists() {
        let file = fs::File::open(&meta_path)?;
        let meta: MapMeta = serde_json::from_reader(io::BufReader::new(file)).map_err(other_err)?;
        if let Some(r) = env_radius {
            if (r - meta.radius_m).abs() > 1e-9 {
                return Err(other_err(format!(
                    "{RADIUS_ENV}={r} differs from the radius in {} ({} m); the cache keeps the radius \
                     it was created with: use another id or remove meta.json",
                    meta_path.display(),
                    meta.radius_m
                )));
            }
        }
        (meta, true)
    } else {
        let (lat, lon, raw_body) = geocode(address)?;
        fs::write(&nominatim_path, raw_body)?;
        let created_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(other_err)?
            .as_secs();
        let meta = MapMeta {
            id: id.to_string(),
            name: name.to_string(),
            center_lat: lat,
            center_lon: lon,
            radius_m: env_radius.unwrap_or(DEFAULT_RADIUS_M),
            tile_size_m: DEFAULT_TILE_M,
            created_unix,
            complete: false,
        };
        fs::write(&meta_path, serde_json::to_string_pretty(&meta).map_err(other_err)?)?;
        (meta, false)
    };

    let osm_dir = raw.join("osm");
    let sleep = |d: Duration| std::thread::sleep(d);
    let client = osm::UreqOverpass;
    let dl = osm::Downloader {
        client: &client,
        sleep: &sleep,
        min_gap: osm::MIN_GAP,
        max_attempts: osm::MAX_ATTEMPTS,
        backoff_base: osm::BACKOFF_BASE,
    };
    let (blocks, blocks_downloaded) = osm::download_missing_blocks(&osm_dir, &meta, &dl)?;

    let (mut roads, mut buildings) = (0, 0);
    osm::visit_ways(&osm_dir, &meta, |e| {
        if e["tags"]["highway"].is_string() {
            roads += 1;
        }
        if e["tags"]["building"].is_string() {
            buildings += 1;
        }
    })?;
    Ok(Summary {
        meta,
        roads,
        buildings,
        geocoding_cached,
        osm_cached: blocks_downloaded == 0,
        blocks,
        blocks_downloaded,
        osm_bytes: dir_bytes(&osm_dir)?,
    })
}

/// Result of `download_osm_areas`: metadata, block counts and the elements of the layer.
#[derive(Debug)]
pub struct AreasSummary {
    pub meta: MapMeta,
    /// Blocks that cover the baked tiles.
    pub blocks: usize,
    /// Blocks downloaded in this run.
    pub blocks_downloaded: usize,
    /// Retries after 429, 502, 503 or 504 in this run.
    pub retries: u32,
    /// Bytes of the blocks written in this run.
    pub bytes_written: u64,
    /// Size of `raw/osm_areas/` after the run, in bytes.
    pub areas_bytes: u64,
    /// Elements per category and per OSM type, over all the blocks.
    pub counts: osm_areas::AreaCounts,
}

/// Downloads the areas, nature and street infrastructure layer (`osm_areas`) for an existing
/// cache entry: `cache_root/<id>/meta.json` gives the center, radius and block grid. Writes
/// `cache_root/<id>/raw/osm_areas/<bx>_<by>.json` and does not touch `raw/osm/`.
///
/// Blocks already on disk are not queried again, so an interrupted run continues where it stopped.
pub fn download_osm_areas(cache_root: &Path, id: &str) -> io::Result<AreasSummary> {
    let dir = cache_root.join(id);
    let meta_path = dir.join("meta.json");
    if !meta_path.exists() {
        return Err(other_err(format!(
            "{} is missing: run download_osm for this id first",
            meta_path.display()
        )));
    }
    let file = fs::File::open(&meta_path)?;
    let meta: MapMeta = serde_json::from_reader(io::BufReader::new(file)).map_err(other_err)?;
    let areas_dir = dir.join("raw").join(osm_areas::DIR_NAME);
    let sleep = |d: Duration| std::thread::sleep(d);
    let client = osm::UreqOverpass;
    let dl = osm::Downloader {
        client: &client,
        sleep: &sleep,
        min_gap: osm::MIN_GAP,
        max_attempts: osm::MAX_ATTEMPTS,
        backoff_base: osm::BACKOFF_BASE,
    };
    let stats = osm_areas::download_missing_blocks(&areas_dir, &meta, &dl)?;
    let counts = osm_areas::count_elements(&areas_dir, &meta)?;
    Ok(AreasSummary {
        meta,
        blocks: stats.blocks,
        blocks_downloaded: stats.downloaded,
        retries: stats.retries,
        bytes_written: stats.bytes_written,
        areas_bytes: dir_bytes(&areas_dir)?,
        counts,
    })
}
