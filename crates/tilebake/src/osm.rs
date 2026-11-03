//! Raw OSM ways in blocks of `BLOCK_M` meters on the local plane. One Overpass
//! query per block, stored in `cache/<id>/raw/osm/<bx>_<by>.json`. Blocks already on
//! disk are not downloaded again, so an interrupted run resumes where it stopped.
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;
use tileformat::MapMeta;

use crate::dem::MapGrid;
use crate::tiles::covering_tiles;

/// Side of one OSM block, in meters. Blocks are cells of a grid anchored at the home,
/// so a block file name means the same area for every radius.
pub const BLOCK_M: f64 = 2000.0;
/// Minimum gap between the start of two Overpass requests.
pub const MIN_GAP: Duration = Duration::from_secs(1);
/// Attempts per block (the first try included) on 429, 502, 503 or 504.
pub const MAX_ATTEMPTS: u32 = 6;
/// Wait before the first retry; doubled on each following retry.
pub const BACKOFF_BASE: Duration = Duration::from_secs(5);

fn other_err<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}

/// Failed HTTP request: the status when the server answered, `None` for transport errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpError {
    pub status: Option<u16>,
    pub message: String,
}

/// Sends an Overpass query (the `data` form field) and returns the response body.
pub trait OverpassClient {
    fn post(&self, query: &str) -> Result<String, HttpError>;
}

/// Overpass over HTTP, with the same User-Agent as the other downloads.
pub struct UreqOverpass;

impl OverpassClient for UreqOverpass {
    fn post(&self, query: &str) -> Result<String, HttpError> {
        match ureq::post(crate::OVERPASS_URL)
            .set("User-Agent", crate::USER_AGENT)
            .send_form(&[("data", query)])
        {
            Ok(resp) => resp.into_string().map_err(|e| HttpError {
                status: None,
                message: e.to_string(),
            }),
            Err(ureq::Error::Status(code, _)) => Err(HttpError {
                status: Some(code),
                message: format!("HTTP {code}"),
            }),
            Err(e) => Err(HttpError {
                status: None,
                message: e.to_string(),
            }),
        }
    }
}

/// How blocks are fetched: client, pacing and retry. `sleep` is injected so tests do not wait.
pub struct Downloader<'a> {
    pub client: &'a dyn OverpassClient,
    pub sleep: &'a dyn Fn(Duration),
    pub min_gap: Duration,
    pub max_attempts: u32,
    pub backoff_base: Duration,
}

impl Downloader<'_> {
    /// Posts `query`, retrying with doubling backoff on 429, 502, 503 and 504. Other errors fail at once.
    pub fn post_with_retry(&self, query: &str) -> io::Result<String> {
        self.post_with_retry_counted(query).map(|(body, _)| body)
    }

    /// Like `post_with_retry`, and also returns how many retries were needed.
    pub fn post_with_retry_counted(&self, query: &str) -> io::Result<(String, u32)> {
        let mut delay = self.backoff_base;
        let mut retries = 0;
        for attempt in 1..=self.max_attempts {
            match self.client.post(query) {
                Ok(body) => return Ok((body, retries)),
                Err(e)
                    if matches!(e.status, Some(429 | 502 | 503 | 504))
                        && attempt < self.max_attempts =>
                {
                    eprintln!(
                        "tilebake: Overpass answered {} (attempt {attempt}/{}), retrying in {delay:?}",
                        e.message, self.max_attempts
                    );
                    (self.sleep)(delay);
                    delay *= 2;
                    retries += 1;
                }
                Err(e) => return Err(other_err(format!("Overpass request failed: {}", e.message))),
            }
        }
        Err(other_err("Overpass: max_attempts is zero"))
    }
}

/// What one run of `download_blocks` did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BlockStats {
    /// Blocks that cover the tiles.
    pub blocks: usize,
    /// Blocks downloaded in this run (the others were already on disk).
    pub downloaded: usize,
    /// Retries after 429, 502, 503 or 504, over all the blocks of this run.
    pub retries: u32,
    /// Bytes of the blocks written in this run.
    pub bytes_written: u64,
}

/// Block indices `(bx, by)` whose area touches the covering tiles of `meta`.
pub fn blocks_for(meta: &MapMeta) -> Vec<(i32, i32)> {
    let tiles = covering_tiles(meta.radius_m, meta.tile_size_m);
    let min_x = tiles.iter().map(|t| t.0).min().unwrap_or(0) as f64 * meta.tile_size_m;
    let max_x = (tiles.iter().map(|t| t.0).max().unwrap_or(0) + 1) as f64 * meta.tile_size_m;
    let min_y = tiles.iter().map(|t| t.1).min().unwrap_or(0) as f64 * meta.tile_size_m;
    let max_y = (tiles.iter().map(|t| t.1).max().unwrap_or(0) + 1) as f64 * meta.tile_size_m;
    let range = |lo: f64, hi: f64| {
        let first = (lo / BLOCK_M).floor() as i32;
        // Last block whose area starts before `hi`: a block that only touches `hi` is left out.
        let last = (hi / BLOCK_M).ceil() as i32 - 1;
        first..=last
    };
    let mut out = Vec::new();
    for by in range(min_y, max_y) {
        for bx in range(min_x, max_x) {
            out.push((bx, by));
        }
    }
    out
}

/// File name of block `(bx, by)` inside `raw/osm/`.
pub fn block_file_name(bx: i32, by: i32) -> String {
    format!("{bx}_{by}.json")
}

/// Overpass query for the ways with `highway` or `building` tags inside the box
/// `(south, west, north, east)`, with full geometry.
pub fn overpass_query(bbox: (f64, f64, f64, f64)) -> String {
    let (s, w, n, e) = bbox;
    format!(
        "[out:json][timeout:90];(way[\"highway\"]({s},{w},{n},{e});way[\"building\"]({s},{w},{n},{e}););out body geom;"
    )
}

/// Geographic box `(south, west, north, east)` of block `(bx, by)`.
pub fn block_bbox(grid: &MapGrid, bx: i32, by: i32) -> (f64, f64, f64, f64) {
    let (x0, y0) = (bx as f64 * BLOCK_M, by as f64 * BLOCK_M);
    let (south, west) = grid.latlon(x0, y0);
    let (north, east) = grid.latlon(x0 + BLOCK_M, y0 + BLOCK_M);
    (south, west, north, east)
}

/// Check of an Overpass body before it is written: it must have an `elements` array.
pub fn check_elements(v: &Value) -> io::Result<()> {
    if v["elements"].is_array() {
        Ok(())
    } else {
        Err(other_err("Overpass response has no 'elements'"))
    }
}

/// Downloads the blocks of `meta` that are not in `dir` yet, one query per block built by
/// `query_for` from the block's bbox. Each block is checked by `check` and written to a
/// `.part` file first, renamed when complete. `layer` names the data in the log lines.
pub fn download_blocks(
    dir: &Path,
    meta: &MapMeta,
    dl: &Downloader,
    layer: &str,
    query_for: &dyn Fn((f64, f64, f64, f64)) -> String,
    check: &dyn Fn(&Value) -> io::Result<()>,
) -> io::Result<BlockStats> {
    fs::create_dir_all(dir)?;
    let grid = MapGrid::from_meta(meta);
    let blocks = blocks_for(meta);
    let mut stats = BlockStats {
        blocks: blocks.len(),
        ..BlockStats::default()
    };
    let mut last_start: Option<Instant> = None;
    for &(bx, by) in &blocks {
        let name = block_file_name(bx, by);
        let path = dir.join(&name);
        if path.exists() {
            continue;
        }
        if let Some(t) = last_start {
            let elapsed = t.elapsed();
            if elapsed < dl.min_gap {
                (dl.sleep)(dl.min_gap - elapsed);
            }
        }
        let query = query_for(block_bbox(&grid, bx, by));
        last_start = Some(Instant::now());
        let (body, retries) = dl.post_with_retry_counted(&query)?;
        stats.retries += retries;
        // Validate before writing, so a bad body never ends up in the cache.
        let v: Value = serde_json::from_str(&body).map_err(other_err)?;
        check(&v).map_err(|e| other_err(format!("{layer} block {name}: {e}")))?;
        let part = dir.join(format!("{name}.part"));
        fs::write(&part, &body)?;
        fs::rename(&part, &path)?;
        stats.downloaded += 1;
        stats.bytes_written += body.len() as u64;
        eprintln!(
            "tilebake: {layer} block {name} downloaded ({} of {}, retries {retries})",
            stats.downloaded,
            blocks.len()
        );
    }
    Ok(stats)
}

/// Downloads the OSM ways of the blocks of `meta` that are not in `dir` yet. Returns `(blocks, downloaded)`.
pub fn download_missing_blocks(
    dir: &Path,
    meta: &MapMeta,
    dl: &Downloader,
) -> io::Result<(usize, usize)> {
    let s = download_blocks(dir, meta, dl, "osm", &overpass_query, &check_elements)?;
    Ok((s.blocks, s.downloaded))
}

/// Calls `f` once for each way of the blocks of `meta` (blocks read in order). A way that
/// crosses a block border is in both files; the first occurrence is the one passed on.
pub fn visit_ways(dir: &Path, meta: &MapMeta, mut f: impl FnMut(&Value)) -> io::Result<()> {
    let mut seen: HashSet<u64> = HashSet::new();
    for (bx, by) in blocks_for(meta) {
        let path = dir.join(block_file_name(bx, by));
        let text = fs::read_to_string(&path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("{}: {e} (download the blocks first)", path.display()),
            )
        })?;
        let v: Value = serde_json::from_str(&text).map_err(other_err)?;
        let elements = v["elements"]
            .as_array()
            .ok_or_else(|| other_err(format!("{}: no 'elements'", path.display())))?;
        for e in elements {
            if let Some(id) = e["id"].as_u64() {
                if !seen.insert(id) {
                    continue;
                }
            }
            f(e);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn meta(radius_m: f64) -> MapMeta {
        MapMeta {
            id: "t".into(),
            name: "t".into(),
            center_lat: -19.9,
            center_lon: -43.9,
            radius_m,
            tile_size_m: 250.0,
            created_unix: 0,
            complete: false,
        }
    }

    /// Scripted client: pops one answer per call and records the queries.
    struct FakeClient {
        answers: RefCell<Vec<Result<String, HttpError>>>,
        queries: RefCell<Vec<String>>,
    }

    impl FakeClient {
        fn new(mut answers: Vec<Result<String, HttpError>>) -> Self {
            answers.reverse();
            FakeClient {
                answers: RefCell::new(answers),
                queries: RefCell::new(Vec::new()),
            }
        }
    }

    impl OverpassClient for FakeClient {
        fn post(&self, query: &str) -> Result<String, HttpError> {
            self.queries.borrow_mut().push(query.to_string());
            self.answers.borrow_mut().pop().unwrap_or_else(|| {
                Err(HttpError {
                    status: None,
                    message: "no more answers".into(),
                })
            })
        }
    }

    fn http(status: u16) -> Result<String, HttpError> {
        Err(HttpError {
            status: Some(status),
            message: format!("HTTP {status}"),
        })
    }

    fn body(ids: &[u64]) -> String {
        let els: Vec<String> = ids
            .iter()
            .map(|id| format!(r#"{{"type":"way","id":{id},"tags":{{"highway":"residential"}},"geometry":[{{"lat":-19.9,"lon":-43.9}},{{"lat":-19.9,"lon":-43.89}}]}}"#))
            .collect();
        format!(r#"{{"elements":[{}]}}"#, els.join(","))
    }

    fn downloader<'a>(
        client: &'a dyn OverpassClient,
        sleep: &'a dyn Fn(Duration),
    ) -> Downloader<'a> {
        Downloader {
            client,
            sleep,
            min_gap: Duration::from_secs(1),
            max_attempts: 3,
            backoff_base: Duration::from_secs(5),
        }
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tilebake-osm-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn one_km_radius_needs_four_blocks() {
        let mut b = blocks_for(&meta(1000.0));
        b.sort();
        assert_eq!(b, vec![(-1, -1), (-1, 0), (0, -1), (0, 0)]);
    }

    #[test]
    fn ten_km_radius_needs_hundred_blocks() {
        let b = blocks_for(&meta(10000.0));
        assert_eq!(b.len(), 100);
        assert_eq!(b.iter().map(|t| t.0).min(), Some(-5));
        assert_eq!(b.iter().map(|t| t.0).max(), Some(4));
    }

    #[test]
    fn block_names_and_bbox_are_consistent() {
        assert_eq!(block_file_name(-1, 3), "-1_3.json");
        let m = meta(1000.0);
        let grid = MapGrid::from_meta(&m);
        // The home is the south-west corner of block (0, 0) and the north-east corner of (-1, -1).
        let (s, w, n, e) = block_bbox(&grid, 0, 0);
        assert!((s - m.center_lat).abs() < 1e-12 && (w - m.center_lon).abs() < 1e-12);
        assert!(n > s && e > w);
        let (_, _, n2, e2) = block_bbox(&grid, -1, -1);
        assert!((n2 - m.center_lat).abs() < 1e-12 && (e2 - m.center_lon).abs() < 1e-12);
        // Block (0, -1) lies south of block (0, 0): its south edge is further south.
        let (s2, _, _, _) = block_bbox(&grid, 0, -1);
        assert!(s2 < s, "{s2} vs {s}");
    }

    #[test]
    fn retry_on_429_then_success_with_doubling_backoff() {
        let client = FakeClient::new(vec![http(429), http(504), Ok("ok".into())]);
        let waits = RefCell::new(Vec::new());
        let sleep = |d: Duration| waits.borrow_mut().push(d);
        let mut dl = downloader(&client, &sleep);
        dl.max_attempts = 5;
        assert_eq!(dl.post_with_retry("q").unwrap(), "ok");
        assert_eq!(client.queries.borrow().len(), 3);
        assert_eq!(
            *waits.borrow(),
            vec![Duration::from_secs(5), Duration::from_secs(10)]
        );
    }

    #[test]
    fn retry_gives_up_after_max_attempts() {
        let client = FakeClient::new(vec![http(429), http(429), http(429)]);
        let sleep = |_: Duration| {};
        let err = downloader(&client, &sleep)
            .post_with_retry("q")
            .unwrap_err();
        assert!(err.to_string().contains("429"), "{err}");
        assert_eq!(client.queries.borrow().len(), 3);
    }

    #[test]
    fn gateway_errors_are_retried() {
        let client = FakeClient::new(vec![http(502), http(503), Ok("ok".into())]);
        let sleep = |_: Duration| {};
        let mut dl = downloader(&client, &sleep);
        dl.max_attempts = 4;
        assert_eq!(dl.post_with_retry("q").unwrap(), "ok");
        assert_eq!(client.queries.borrow().len(), 3);
    }

    #[test]
    fn other_status_fails_without_retry() {
        let client = FakeClient::new(vec![http(400), Ok("never".into())]);
        let calls = Cell::new(0);
        let sleep = |_: Duration| calls.set(calls.get() + 1);
        assert!(downloader(&client, &sleep).post_with_retry("q").is_err());
        assert_eq!(client.queries.borrow().len(), 1);
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn download_skips_blocks_on_disk_and_paces_requests() {
        let dir = temp_dir("skip");
        let m = meta(1000.0);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(block_file_name(-1, -1)), body(&[1])).unwrap();
        let client = FakeClient::new(vec![Ok(body(&[2])), Ok(body(&[3])), Ok(body(&[4]))]);
        let waits = RefCell::new(Vec::new());
        let sleep = |d: Duration| waits.borrow_mut().push(d);
        let (blocks, downloaded) =
            download_missing_blocks(&dir, &m, &downloader(&client, &sleep)).unwrap();
        assert_eq!((blocks, downloaded), (4, 3));
        assert_eq!(client.queries.borrow().len(), 3);
        // Between two requests the wait is the gap minus the time already elapsed: never 0.
        let w = waits.borrow();
        assert_eq!(w.len(), 2);
        assert!(w
            .iter()
            .all(|d| *d > Duration::from_millis(900) && *d <= Duration::from_secs(1)));
        assert!(!dir.join(format!("{}.part", block_file_name(0, 0))).exists());
        // Second run: nothing left to download.
        let client2 = FakeClient::new(vec![]);
        let (_, again) = download_missing_blocks(&dir, &m, &downloader(&client2, &sleep)).unwrap();
        assert_eq!(again, 0);
        assert!(client2.queries.borrow().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_body_is_not_written() {
        let dir = temp_dir("invalid");
        let m = meta(1000.0);
        let client = FakeClient::new(vec![Ok("<html>busy</html>".into())]);
        let sleep = |_: Duration| {};
        assert!(download_missing_blocks(&dir, &m, &downloader(&client, &sleep)).is_err());
        assert!(!dir.join(block_file_name(-1, -1)).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn visit_ways_dedupes_by_id_across_blocks() {
        let dir = temp_dir("dedupe");
        let m = meta(1000.0);
        fs::create_dir_all(&dir).unwrap();
        // Way 7 crosses the border between two blocks and is in both files.
        for (bx, by) in blocks_for(&m) {
            let ids: Vec<u64> = match (bx, by) {
                (-1, -1) => vec![7, 1],
                (0, -1) => vec![7, 2],
                _ => vec![3],
            };
            fs::write(dir.join(block_file_name(bx, by)), body(&ids)).unwrap();
        }
        let mut ids = Vec::new();
        visit_ways(&dir, &m, |e| ids.push(e["id"].as_u64().unwrap())).unwrap();
        ids.sort();
        assert_eq!(ids, vec![1, 2, 3, 7]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn visit_ways_reports_missing_block() {
        let dir = temp_dir("missing");
        fs::create_dir_all(&dir).unwrap();
        let err = visit_ways(&dir, &meta(1000.0), |_| {}).unwrap_err();
        assert!(
            err.to_string().contains("download the blocks first"),
            "{err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
