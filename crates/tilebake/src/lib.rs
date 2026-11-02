//! OSM + DEM -> tiles pipeline. Called by the GDExtension on its own thread.
use serde_json::Value;
use std::fs;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tileformat::MapMeta;

pub mod dem;

const USER_AGENT: &str = "driving-the-earth/0.1 (tilebake; projeto pessoal)";
const NOMINATIM_URL: &str = "https://nominatim.openstreetmap.org/search";
const OVERPASS_URL: &str = "https://overpass-api.de/api/interpreter";

/// Collection radius around the home, in meters.
pub const DEFAULT_RADIUS_M: f64 = 1000.0;
/// Side of a grid tile, in meters.
pub const DEFAULT_TILE_M: f64 = 250.0;

/// Result of `download_osm`: metadata and raw counts.
#[derive(Debug)]
pub struct Summary {
    pub meta: MapMeta,
    /// Ways with a `highway` tag.
    pub roads: usize,
    /// Ways with a `building` tag.
    pub buildings: usize,
    /// Geocoding came from an existing `meta.json`.
    pub geocoding_cached: bool,
    /// Raw Overpass data came from an existing `raw/overpass.json`.
    pub osm_cached: bool,
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

/// Counts (roads, buildings) in a raw Overpass response.
fn count_ways(overpass_json: &str) -> io::Result<(usize, usize)> {
    let v: Value = serde_json::from_str(overpass_json).map_err(other_err)?;
    let elements = v["elements"]
        .as_array()
        .ok_or_else(|| other_err("Overpass response has no 'elements'"))?;
    let roads = elements.iter().filter(|e| e["tags"]["highway"].is_string()).count();
    let buildings = elements.iter().filter(|e| e["tags"]["building"].is_string()).count();
    Ok((roads, buildings))
}

/// Downloads the OSM data around the address and writes the raw data to `cache_root/<id>/raw/`.
///
/// Reuses the cache: if `meta.json` exists, it does not geocode again; if
/// `raw/overpass.json` exists, it does not query Overpass again.
pub fn download_osm(cache_root: &Path, id: &str, name: &str, address: &str) -> io::Result<Summary> {
    let dir = cache_root.join(id);
    let raw = dir.join("raw");
    fs::create_dir_all(&raw)?;
    let meta_path = dir.join("meta.json");
    let nominatim_path = raw.join("nominatim.json");
    let overpass_path = raw.join("overpass.json");

    let (meta, geocoding_cached) = if meta_path.exists() {
        let file = fs::File::open(&meta_path)?;
        let meta: MapMeta = serde_json::from_reader(io::BufReader::new(file)).map_err(other_err)?;
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
            radius_m: DEFAULT_RADIUS_M,
            tile_size_m: DEFAULT_TILE_M,
            created_unix,
            complete: false,
        };
        fs::write(&meta_path, serde_json::to_string_pretty(&meta).map_err(other_err)?)?;
        (meta, false)
    };

    let (osm_json, osm_cached) = if overpass_path.exists() {
        (fs::read_to_string(&overpass_path)?, true)
    } else {
        let query = format!(
            "[out:json][timeout:90];(way[\"highway\"](around:{r},{lat},{lon});way[\"building\"](around:{r},{lat},{lon}););out body geom;",
            r = meta.radius_m,
            lat = meta.center_lat,
            lon = meta.center_lon,
        );
        let body = ureq::post(OVERPASS_URL)
            .set("User-Agent", USER_AGENT)
            .send_form(&[("data", query.as_str())])
            .map_err(other_err)?
            .into_string()?;
        count_ways(&body)?; // validate before writing
        fs::write(&overpass_path, &body)?;
        (body, false)
    };

    let (roads, buildings) = count_ways(&osm_json)?;
    Ok(Summary {
        meta,
        roads,
        buildings,
        geocoding_cached,
        osm_cached,
    })
}
