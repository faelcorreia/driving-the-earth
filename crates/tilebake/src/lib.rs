//! OSM + DEM -> tiles pipeline. Called by the GDExtension on its own thread.
//!
//! Two entry points run the whole pipeline: `bake_region` (any lat/lon, with cancellation and
//! progress) and `download_osm` (geocodes an address, then downloads the OSM blocks). Both reuse
//! the cache: a step whose output is on disk is skipped.
use serde_json::Value;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tileformat::MapMeta;

pub mod clip;
pub mod dem;
pub mod far_lights;
pub mod home;
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
/// Environment variable with the collection radius, in meters (read when `meta.json` is created).
const RADIUS_ENV: &str = "TILEBAKE_RADIUS_M";

/// Collection radius around the home, in meters, when `TILEBAKE_RADIUS_M` is not set.
pub const DEFAULT_RADIUS_M: f64 = 10000.0;
/// Side of a grid tile, in meters.
pub const DEFAULT_TILE_M: f64 = tileformat::units::TILE_M;

/// Latitude limit of `bake_region`: the local map plane (equirectangular, centered on the map)
/// degenerates near the poles.
pub const MAX_ABS_LAT: f64 = 85.0;
/// Longest accepted id (it is a folder name under the cache root).
pub const MAX_ID_LEN: usize = 64;

/// Stage keys reported by `Progress` (stable identifiers, not text for the UI).
pub const STAGE_OSM: &str = "osm";
pub const STAGE_OSM_AREAS: &str = "osm_areas";
pub const STAGE_DEM: &str = "dem";
pub const STAGE_TILES: &str = "tiles";
pub const STAGE_ROADS: &str = "roads";
pub const STAGE_FAR_LIGHTS: &str = "far_lights";

/// One progress report: `done` of `total` units of `stage` are finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub stage: &'static str,
    pub done: usize,
    pub total: usize,
}

/// Cancellation flag and progress callback of a run. Cancellation is checked between steps,
/// and between blocks and tiles; a cancelled run returns `ErrorKind::Interrupted` and leaves
/// on disk what it has finished (the next run continues from there).
pub struct Hooks<'a> {
    pub cancel: &'a AtomicBool,
    pub progress: &'a mut dyn FnMut(Progress),
}

impl Hooks<'_> {
    /// `Err(Interrupted)` when the cancel flag is set.
    pub fn check(&self) -> io::Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(io::Error::new(io::ErrorKind::Interrupted, "bake cancelled"))
        } else {
            Ok(())
        }
    }

    pub fn report(&mut self, stage: &'static str, done: usize, total: usize) {
        (self.progress)(Progress { stage, done, total });
    }
}

/// Never set: the hooks of the entry points that do not take a flag.
static NEVER_CANCEL: AtomicBool = AtomicBool::new(false);

/// Runs `f` with hooks that never cancel and report nothing.
fn run_quiet<R>(f: impl FnOnce(&mut Hooks<'_>) -> R) -> R {
    let mut noop = |_: Progress| {};
    let mut hooks = Hooks {
        cancel: &NEVER_CANCEL,
        progress: &mut noop,
    };
    f(&mut hooks)
}

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

/// Result of `bake_region`: the summary of each stage.
#[derive(Debug)]
pub struct RegionSummary {
    pub osm: Summary,
    pub areas: AreasSummary,
    pub tiles: tiles::BakeSummary,
    pub far_lights: far_lights::Summary,
}

fn other_err<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}

fn invalid_input<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, e.to_string())
}

/// Checks a latitude and longitude in degrees: finite, `|lat| <= MAX_ABS_LAT`, `|lon| <= 180`.
pub fn validate_coords(lat: f64, lon: f64) -> io::Result<()> {
    if !lat.is_finite() || !lon.is_finite() {
        return Err(invalid_input("coordinates must be finite numbers"));
    }
    if lat.abs() > MAX_ABS_LAT {
        return Err(invalid_input(format!(
            "latitude {lat} is outside -{MAX_ABS_LAT}..{MAX_ABS_LAT} (the local map plane is not defined near the poles)"
        )));
    }
    if lon.abs() > 180.0 {
        return Err(invalid_input(format!(
            "longitude {lon} is outside -180..180"
        )));
    }
    Ok(())
}

/// Checks an id: 1 to `MAX_ID_LEN` characters, ASCII letters, digits, `-` and `_` only.
/// The id is a folder name under the cache root, so nothing that could leave it is accepted.
pub fn check_id(id: &str) -> io::Result<()> {
    if id.is_empty() {
        return Err(invalid_input("id is empty"));
    }
    if id.len() > MAX_ID_LEN {
        return Err(invalid_input(format!(
            "id is longer than {MAX_ID_LEN} characters"
        )));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(invalid_input(format!(
            "id '{id}' may use only ASCII letters, digits, '-' and '_'"
        )));
    }
    Ok(())
}

/// Default id for a point, for example `at_m19p9596_m43p9090` for (-19.9596, -43.9090).
pub fn id_for_coords(lat: f64, lon: f64) -> String {
    let part = |v: f64| format!("{v:.4}").replace('-', "m").replace('.', "p");
    format!("at_{}_{}", part(lat), part(lon))
}

/// Checks everything `bake_region` needs before touching the disk or the network.
fn validate_request(id: &str, lat: f64, lon: f64, radius_m: f64) -> io::Result<()> {
    check_id(id)?;
    validate_coords(lat, lon)?;
    if !(radius_m.is_finite() && radius_m > 0.0) {
        return Err(invalid_input("radius must be a positive number of meters"));
    }
    Ok(())
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
pub fn radius_from_env() -> io::Result<Option<f64>> {
    match std::env::var(RADIUS_ENV) {
        Ok(text) => {
            let r: f64 = text
                .trim()
                .parse()
                .map_err(|_| other_err(format!("{RADIUS_ENV} is not a number: {text}")))?;
            if r > 0.0 && r.is_finite() {
                Ok(Some(r))
            } else {
                Err(other_err(format!(
                    "{RADIUS_ENV} must be a positive number of meters"
                )))
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

/// Where the center of a new cache entry comes from.
enum Origin<'a> {
    /// Geocoded with Nominatim (the raw response is kept in `raw/nominatim.json`).
    Address(&'a str),
    /// Given directly, in degrees.
    Coords(f64, f64),
}

/// Reads `dir/meta.json`, or creates it from `origin` when it does not exist. Returns the
/// metadata and whether it came from the cache. With an existing `meta.json`, a `radius_m` or
/// a coordinate origin that differs from it is an error: the cache keeps what it was created with.
fn open_or_create_meta(
    dir: &Path,
    id: &str,
    name: &str,
    origin: &Origin,
    radius_m: Option<f64>,
) -> io::Result<(MapMeta, bool)> {
    let raw = dir.join("raw");
    fs::create_dir_all(&raw)?;
    let meta_path = dir.join("meta.json");
    if meta_path.exists() {
        let file = fs::File::open(&meta_path)?;
        let meta: MapMeta = serde_json::from_reader(io::BufReader::new(file)).map_err(other_err)?;
        if let Some(r) = radius_m {
            if (r - meta.radius_m).abs() > 1e-9 {
                return Err(invalid_input(format!(
                    "radius {r} m differs from the radius in {} ({} m); the cache keeps the radius \
                     it was created with: use another id or remove meta.json",
                    meta_path.display(),
                    meta.radius_m
                )));
            }
        }
        if let Origin::Coords(lat, lon) = origin {
            if (lat - meta.center_lat).abs() > 1e-9 || (lon - meta.center_lon).abs() > 1e-9 {
                return Err(invalid_input(format!(
                    "{} is centered at ({:.6}, {:.6}), not at ({lat:.6}, {lon:.6}): use another id or remove the cache folder",
                    meta_path.display(),
                    meta.center_lat,
                    meta.center_lon
                )));
            }
        }
        return Ok((meta, true));
    }
    let (lat, lon, nominatim_body) = match origin {
        Origin::Address(address) => {
            let (lat, lon, body) = geocode(address)?;
            (lat, lon, Some(body))
        }
        Origin::Coords(lat, lon) => (*lat, *lon, None),
    };
    if let Some(body) = nominatim_body {
        fs::write(raw.join("nominatim.json"), body)?;
    }
    let created_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(other_err)?
        .as_secs();
    let meta = MapMeta {
        id: id.to_string(),
        name: name.to_string(),
        center_lat: lat,
        center_lon: lon,
        radius_m: radius_m.unwrap_or(DEFAULT_RADIUS_M),
        tile_size_m: DEFAULT_TILE_M,
        created_unix,
        complete: false,
    };
    fs::write(
        &meta_path,
        serde_json::to_string_pretty(&meta).map_err(other_err)?,
    )?;
    Ok((meta, false))
}

fn read_meta(path: &Path) -> io::Result<MapMeta> {
    let file = fs::File::open(path)?;
    serde_json::from_reader(io::BufReader::new(file)).map_err(other_err)
}

/// The Overpass downloader with the production pacing and retries.
fn downloader<'a>(
    client: &'a dyn osm::OverpassClient,
    sleep: &'a dyn Fn(Duration),
) -> osm::Downloader<'a> {
    osm::Downloader {
        client,
        sleep,
        min_gap: osm::MIN_GAP,
        max_attempts: osm::MAX_ATTEMPTS,
        backoff_base: osm::BACKOFF_BASE,
    }
}

/// Downloads the OSM blocks of `meta` (missing ones only) and counts the ways.
fn osm_stage(
    dir: &Path,
    meta: MapMeta,
    geocoding_cached: bool,
    dl: &osm::Downloader,
    hooks: &mut Hooks,
) -> io::Result<Summary> {
    let osm_dir = dir.join("raw").join("osm");
    let (blocks, blocks_downloaded) =
        osm::download_missing_blocks_with(&osm_dir, &meta, dl, hooks)?;
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

/// Shared body of `download_osm` and `download_osm_at`.
fn osm_for(
    cache_root: &Path,
    id: &str,
    name: &str,
    origin: &Origin,
    radius_m: Option<f64>,
    dl: &osm::Downloader,
    hooks: &mut Hooks,
) -> io::Result<Summary> {
    let dir = cache_root.join(id);
    let (meta, geocoding_cached) = open_or_create_meta(&dir, id, name, origin, radius_m)?;
    osm_stage(&dir, meta, geocoding_cached, dl, hooks)
}

/// Downloads the OSM data around the address and writes it to `cache_root/<id>/raw/osm/`,
/// one file per block of `osm::BLOCK_M` meters (see `osm`).
///
/// Reuses the cache: if `meta.json` exists, it does not geocode again and keeps its radius
/// (a different `TILEBAKE_RADIUS_M` is an error); blocks already on disk are not queried again,
/// so an interrupted run continues where it stopped.
pub fn download_osm(cache_root: &Path, id: &str, name: &str, address: &str) -> io::Result<Summary> {
    let radius = radius_from_env()?;
    let client = osm::UreqOverpass;
    let sleep = |d: Duration| std::thread::sleep(d);
    let dl = downloader(&client, &sleep);
    run_quiet(|h| {
        osm_for(
            cache_root,
            id,
            name,
            &Origin::Address(address),
            radius,
            &dl,
            h,
        )
    })
}

/// Like `download_osm`, centered on (`lat`, `lon`) in degrees instead of an address: no
/// geocoding, and the cache must be centered there if it already exists.
pub fn download_osm_at(
    cache_root: &Path,
    id: &str,
    name: &str,
    lat: f64,
    lon: f64,
    radius_m: f64,
) -> io::Result<Summary> {
    validate_request(id, lat, lon, radius_m)?;
    let client = osm::UreqOverpass;
    let sleep = |d: Duration| std::thread::sleep(d);
    let dl = downloader(&client, &sleep);
    run_quiet(|h| {
        osm_for(
            cache_root,
            id,
            name,
            &Origin::Coords(lat, lon),
            Some(radius_m),
            &dl,
            h,
        )
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

/// Downloads the areas blocks of `meta` (missing ones only) and counts their elements.
fn areas_stage(
    dir: &Path,
    meta: MapMeta,
    dl: &osm::Downloader,
    hooks: &mut Hooks,
) -> io::Result<AreasSummary> {
    let areas_dir = dir.join("raw").join(osm_areas::DIR_NAME);
    let stats = osm_areas::download_missing_blocks_with(&areas_dir, &meta, dl, hooks)?;
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
    let meta = read_meta(&meta_path)?;
    let client = osm::UreqOverpass;
    let sleep = |d: Duration| std::thread::sleep(d);
    let dl = downloader(&client, &sleep);
    run_quiet(|h| areas_stage(&dir, meta, &dl, h))
}

/// Bakes the whole map of a region centered on (`lat`, `lon`), in degrees, with radius `radius_m`
/// (meters): OSM blocks, areas layer, DEM and tiles (with the roads file), then far lights.
/// Needs no address and no environment variables. Each step is skipped when its output is on
/// disk, as in the CLI. `progress` is called per stage and per block or tile; `cancel` is checked
/// between steps and between blocks and tiles. A cancelled run returns `ErrorKind::Interrupted`.
///
/// The id is a folder name under `cache_root`: see `check_id`. The coordinates and radius are
/// checked by `validate_coords` and `validate_request` before any file or network access.
pub fn bake_region(
    cache_root: &Path,
    id: &str,
    name: &str,
    lat: f64,
    lon: f64,
    radius_m: f64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> io::Result<RegionSummary> {
    let mut hooks = Hooks { cancel, progress };
    bake_region_with(cache_root, id, name, lat, lon, radius_m, &mut hooks)
}

fn bake_region_with(
    cache_root: &Path,
    id: &str,
    name: &str,
    lat: f64,
    lon: f64,
    radius_m: f64,
    hooks: &mut Hooks,
) -> io::Result<RegionSummary> {
    validate_request(id, lat, lon, radius_m)?;
    hooks.check()?;
    let dir = cache_root.join(id);
    let client = osm::UreqOverpass;
    let sleep = |d: Duration| std::thread::sleep(d);
    let dl = downloader(&client, &sleep);

    let (meta, geocoding_cached) =
        open_or_create_meta(&dir, id, name, &Origin::Coords(lat, lon), Some(radius_m))?;
    let osm = osm_stage(&dir, meta, geocoding_cached, &dl, hooks)?;
    hooks.check()?;
    let areas = areas_stage(&dir, read_meta(&dir.join("meta.json"))?, &dl, hooks)?;
    hooks.check()?;
    let tiles = tiles::bake_tiles_with(cache_root, id, hooks)?;
    hooks.check()?;
    hooks.report(STAGE_FAR_LIGHTS, 0, 1);
    let far_lights = far_lights::bake_far_lights(cache_root, id)?;
    hooks.report(STAGE_FAR_LIGHTS, 1, 1);
    Ok(RegionSummary {
        osm,
        areas,
        tiles,
        far_lights,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osm::{HttpError, OverpassClient};
    use std::cell::Cell;
    use std::path::PathBuf;

    /// Counts the requests and answers an empty Overpass body, or `fail` for every request.
    struct FakeClient {
        posts: Cell<u32>,
        fail: Option<HttpError>,
    }

    impl OverpassClient for FakeClient {
        fn post(&self, _query: &str) -> Result<String, HttpError> {
            self.posts.set(self.posts.get() + 1);
            match &self.fail {
                Some(e) => Err(e.clone()),
                None => Ok("{\"elements\":[]}".to_string()),
            }
        }
    }

    fn fake(fail: Option<HttpError>) -> FakeClient {
        FakeClient {
            posts: Cell::new(0),
            fail,
        }
    }

    fn no_sleep(_: Duration) {}

    fn fake_downloader<'a>(
        client: &'a FakeClient,
        sleep: &'a dyn Fn(Duration),
    ) -> osm::Downloader<'a> {
        osm::Downloader {
            client,
            sleep,
            min_gap: Duration::ZERO,
            max_attempts: 1,
            backoff_base: Duration::ZERO,
        }
    }

    fn meta_fixture(id: &str) -> MapMeta {
        MapMeta {
            id: id.to_string(),
            name: "Test".to_string(),
            center_lat: -19.92,
            center_lon: -43.94,
            radius_m: 2500.0,
            tile_size_m: DEFAULT_TILE_M,
            created_unix: 0,
            complete: false,
        }
    }

    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("tilebake-lib-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn progress_reports_each_block_in_order() {
        let root = temp_root("progress");
        let meta = meta_fixture("p");
        let expected = osm::blocks_for(&meta).len();
        assert!(expected > 1, "fixture should cover several blocks");
        let client = fake(None);
        let sleep = no_sleep;
        let dl = fake_downloader(&client, &sleep);
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let mut cb = |p: Progress| seen.push(p);
        let mut hooks = Hooks {
            cancel: &cancel,
            progress: &mut cb,
        };
        let (blocks, downloaded) =
            osm::download_missing_blocks_with(&root.join("osm"), &meta, &dl, &mut hooks).unwrap();
        assert_eq!((blocks, downloaded), (expected, expected));
        assert_eq!(seen.len(), expected);
        for (i, p) in seen.iter().enumerate() {
            assert_eq!(p.stage, STAGE_OSM);
            assert_eq!(p.done, i + 1);
            assert_eq!(p.total, expected);
        }
        // A second run skips every block on disk and still reports each one.
        let client2 = fake(None);
        let dl2 = fake_downloader(&client2, &sleep);
        let mut again = Vec::new();
        let mut cb2 = |p: Progress| again.push(p);
        let mut hooks2 = Hooks {
            cancel: &cancel,
            progress: &mut cb2,
        };
        osm::download_missing_blocks_with(&root.join("osm"), &meta, &dl2, &mut hooks2).unwrap();
        assert_eq!(client2.posts.get(), 0);
        assert_eq!(again.len(), expected);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cancel_before_start_makes_no_request() {
        let root = temp_root("cancel-start");
        let meta = meta_fixture("c");
        let client = fake(None);
        let sleep = no_sleep;
        let dl = fake_downloader(&client, &sleep);
        let cancel = AtomicBool::new(true);
        let mut noop = |_: Progress| {};
        let mut hooks = Hooks {
            cancel: &cancel,
            progress: &mut noop,
        };
        let err = osm::download_missing_blocks_with(&root.join("osm"), &meta, &dl, &mut hooks)
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert_eq!(client.posts.get(), 0);
    }

    #[test]
    fn cancel_between_blocks_stops_after_the_current_one() {
        let root = temp_root("cancel-mid");
        let meta = meta_fixture("m");
        let client = fake(None);
        let sleep = no_sleep;
        let dl = fake_downloader(&client, &sleep);
        let cancel = AtomicBool::new(false);
        let mut cb = |_: Progress| cancel.store(true, Ordering::Relaxed);
        let mut hooks = Hooks {
            cancel: &cancel,
            progress: &mut cb,
        };
        let err = osm::download_missing_blocks_with(&root.join("osm"), &meta, &dl, &mut hooks)
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert_eq!(
            client.posts.get(),
            1,
            "only the block in progress is fetched"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cancelled_bake_region_returns_interrupted_without_touching_disk() {
        let root = temp_root("region-cancel");
        let cancel = AtomicBool::new(true);
        let err = bake_region(
            &root,
            "casa2",
            "x",
            -19.92,
            -43.94,
            2500.0,
            &cancel,
            &mut |_| {},
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert!(!root.join("casa2").exists());
    }

    #[test]
    fn network_error_is_an_io_error_with_the_message() {
        let root = temp_root("network");
        let meta = meta_fixture("n");
        let client = fake(Some(HttpError {
            status: None,
            message: "connection refused".to_string(),
        }));
        let sleep = no_sleep;
        let dl = fake_downloader(&client, &sleep);
        let cancel = AtomicBool::new(false);
        let mut noop = |_: Progress| {};
        let mut hooks = Hooks {
            cancel: &cancel,
            progress: &mut noop,
        };
        let err = osm::download_missing_blocks_with(&root.join("osm"), &meta, &dl, &mut hooks)
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Other);
        assert!(err.to_string().contains("connection refused"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn coordinates_are_validated() {
        assert!(validate_coords(-19.92, -43.94).is_ok());
        assert!(validate_coords(0.0, 0.0).is_ok());
        assert!(validate_coords(85.0, 180.0).is_ok());
        for (lat, lon) in [
            (91.0, 0.0),
            (-85.5, 0.0),
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
            (0.0, 180.5),
            (0.0, -181.0),
        ] {
            let err = validate_coords(lat, lon).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "({lat}, {lon})");
        }
    }

    #[test]
    fn bake_region_rejects_bad_input_before_any_access() {
        let root = temp_root("region-invalid");
        let cancel = AtomicBool::new(false);
        let bad = [
            ("casa", 95.0, 0.0, 2500.0),
            ("casa", -19.9, -43.9, 0.0),
            ("casa", -19.9, -43.9, f64::NAN),
            ("../fora", -19.9, -43.9, 2500.0),
            ("", -19.9, -43.9, 2500.0),
        ];
        for (id, lat, lon, r) in bad {
            let err = bake_region(&root, id, "x", lat, lon, r, &cancel, &mut |_| {}).unwrap_err();
            assert_eq!(
                err.kind(),
                io::ErrorKind::InvalidInput,
                "{id} ({lat}, {lon}, {r})"
            );
        }
        assert!(!root.exists());
    }

    #[test]
    fn existing_cache_centered_elsewhere_is_an_error() {
        let root = temp_root("region-center");
        let dir = root.join("casa3");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("meta.json"),
            serde_json::to_string(&meta_fixture("casa3")).unwrap(),
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        // Far from the cached center: refused before any network access.
        let err = bake_region(
            &root,
            "casa3",
            "x",
            10.0,
            10.0,
            2500.0,
            &cancel,
            &mut |_| {},
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("centered at"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ids_are_checked() {
        for ok in [
            "casa",
            "at_m19p9596_m43p9090",
            "a-b_C9",
            &"x".repeat(MAX_ID_LEN),
        ] {
            assert!(check_id(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            ".",
            "..",
            "a/b",
            "a b",
            "a.b",
            "é",
            &"x".repeat(MAX_ID_LEN + 1),
        ] {
            assert_eq!(
                check_id(bad).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "{bad}"
            );
        }
    }

    #[test]
    fn default_id_for_a_point_is_a_valid_id() {
        let id = id_for_coords(-19.9596, -43.9090);
        assert_eq!(id, "at_m19p9596_m43p9090");
        assert!(check_id(&id).is_ok());
        assert!(check_id(&id_for_coords(0.0, 0.0)).is_ok());
    }
}
