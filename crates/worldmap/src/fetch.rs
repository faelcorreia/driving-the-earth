//! Download and disk cache of OpenFreeMap vector tiles (F17b3). The TileJSON of the planet gives
//! the versioned tile URL template; each raw `.pbf` tile is one file named after the version and
//! its z/x/y, kept until the cache is over its cap (the oldest files go first). The HTTP client
//! and the sleep are injected, so tests never touch the network.
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::tiles::TileId;

/// Disk cap for the raw tiles, in bytes.
pub const CACHE_CAP_BYTES: u64 = 500 * 1024 * 1024;
/// Identifies the app to the tile server, as the other downloads do.
pub const USER_AGENT: &str = "driving-the-earth/0.2 (home picker)";
/// OpenFreeMap's planet TileJSON (free, no key).
pub const TILEJSON_URL: &str = "https://tiles.openfreemap.org/planet";
/// Requests per resource, the first included. Retries wait 2 s, 4 s, 8 s.
pub const MAX_ATTEMPTS: u32 = 4;
pub const BACKOFF_BASE: Duration = Duration::from_secs(2);
/// Last template that worked, kept for offline use.
const TEMPLATE_FILE: &str = "tilejson.txt";

/// Where a tile came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Cache,
    Network,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpError {
    /// HTTP status, or `None` when there was no response (offline, timeout).
    pub status: Option<u16>,
    pub message: String,
}

/// Downloads one URL and returns its body.
pub trait TileClient {
    fn get(&self, url: &str) -> Result<Vec<u8>, HttpError>;
}

/// The real client: ureq, with the app's User-Agent.
pub struct UreqTiles;

impl TileClient for UreqTiles {
    fn get(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        match ureq::get(url)
            .set("User-Agent", USER_AGENT)
            .timeout(Duration::from_secs(20))
            .call()
        {
            Ok(resp) => {
                let mut body = Vec::new();
                resp.into_reader()
                    .read_to_end(&mut body)
                    .map_err(|e| HttpError {
                        status: None,
                        message: e.to_string(),
                    })?;
                Ok(body)
            }
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

/// Retried: no response, 429 and 5xx. Other statuses fail at once.
fn retryable(e: &HttpError) -> bool {
    match e.status {
        None | Some(429) => true,
        Some(code) => code >= 500,
    }
}

/// GET with up to `MAX_ATTEMPTS` tries and doubling waits.
pub fn get_with_retry(
    client: &dyn TileClient,
    url: &str,
    sleep: &dyn Fn(Duration),
) -> Result<Vec<u8>, HttpError> {
    let mut attempt = 1;
    loop {
        match client.get(url) {
            Ok(body) => return Ok(body),
            Err(e) if attempt < MAX_ATTEMPTS && retryable(&e) => {
                sleep(BACKOFF_BASE * 2u32.pow(attempt - 1));
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// The tile URL template (`.../{z}/{x}/{y}.pbf`) from a TileJSON body. Only https templates.
pub fn template_from_tilejson(body: &str) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let t = v["tiles"]
        .get(0)
        .and_then(|t| t.as_str())
        .ok_or_else(|| "TileJSON without tiles".to_string())?;
    if !t.starts_with("https://") || !["{z}", "{x}", "{y}"].iter().all(|p| t.contains(p)) {
        return Err(format!("unexpected tile template: {t}"));
    }
    Ok(t.to_string())
}

/// The version of a template: the path segment before `{z}`, e.g. `20261101_080013_pt`.
pub fn version_of(template: &str) -> String {
    let parts: Vec<&str> = template.split('/').collect();
    match parts.iter().position(|p| *p == "{z}") {
        Some(i) if i > 0 => parts[i - 1].to_string(),
        _ => "unversioned".to_string(),
    }
}

/// The TileJSON is asked for once per session by the caller. On failure, the last template saved
/// in `dir` is used (offline). Success saves the template there.
pub fn load_template(
    dir: &Path,
    client: &dyn TileClient,
    sleep: &dyn Fn(Duration),
) -> Result<String, String> {
    let fetched = get_with_retry(client, TILEJSON_URL, sleep)
        .map_err(|e| e.message)
        .and_then(|b| String::from_utf8(b).map_err(|e| e.to_string()))
        .and_then(|s| template_from_tilejson(&s));
    match fetched {
        Ok(template) => {
            let _ = write_atomic(dir, &dir.join(TEMPLATE_FILE), template.as_bytes());
            Ok(template)
        }
        Err(e) => match fs::read_to_string(dir.join(TEMPLATE_FILE)) {
            Ok(saved) if template_from_tilejson_path(&saved) => Ok(saved.trim().to_string()),
            _ => Err(e),
        },
    }
}

fn template_from_tilejson_path(t: &str) -> bool {
    let t = t.trim();
    t.starts_with("https://") && ["{z}", "{x}", "{y}"].iter().all(|p| t.contains(p))
}

pub fn tile_url(template: &str, id: TileId) -> String {
    template
        .replace("{z}", &id.z.to_string())
        .replace("{x}", &id.x.to_string())
        .replace("{y}", &id.y.to_string())
}

/// File name of a tile in the cache: version, then z, x, y.
pub fn cache_file_name(template: &str, id: TileId) -> String {
    format!("{}_{}_{}_{}.pbf", version_of(template), id.z, id.x, id.y)
}

/// Returns the raw bytes of a tile: from `dir` if cached, otherwise downloaded (with retry) and
/// written there. A failed download leaves nothing on disk.
pub fn load_tile(
    dir: &Path,
    template: &str,
    id: TileId,
    client: &dyn TileClient,
    sleep: &dyn Fn(Duration),
) -> Result<(Vec<u8>, Source), String> {
    let path = dir.join(cache_file_name(template, id));
    if let Ok(bytes) = fs::read(&path) {
        if !bytes.is_empty() {
            return Ok((bytes, Source::Cache));
        }
    }
    let body = get_with_retry(client, &tile_url(template, id), sleep).map_err(|e| e.message)?;
    write_atomic(dir, &path, &body).map_err(|e| e.to_string())?;
    Ok((body, Source::Network))
}

/// Path of a cached tile, so a corrupt file can be removed.
pub fn cache_path(dir: &Path, template: &str, id: TileId) -> PathBuf {
    dir.join(cache_file_name(template, id))
}

fn write_atomic(dir: &Path, path: &Path, body: &[u8]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let tmp = path.with_extension("part");
    fs::write(&tmp, body)?;
    fs::rename(&tmp, path)
}

/// One file of the cache, for eviction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub bytes: u64,
    /// Seconds since the epoch of the last write.
    pub modified: u64,
}

/// Names to delete, oldest first, so that the rest adds up to at most `cap` bytes.
pub fn evict_plan(entries: &[Entry], cap: u64) -> Vec<String> {
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|a, b| (a.modified, &a.name).cmp(&(b.modified, &b.name)));
    let mut total: u64 = entries.iter().map(|e| e.bytes).sum();
    let mut out = Vec::new();
    for e in sorted {
        if total <= cap {
            break;
        }
        total -= e.bytes;
        out.push(e.name.clone());
    }
    out
}

/// Deletes the oldest `.pbf` tiles of `dir` until the tiles add up to at most `cap`. Returns how
/// many files were removed. A missing directory is not an error.
pub fn enforce_cap(dir: &Path, cap: u64) -> io::Result<usize> {
    let read = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut entries = Vec::new();
    for item in read {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        let meta = item.metadata()?;
        if !meta.is_file() || !name.ends_with(".pbf") {
            continue;
        }
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        entries.push(Entry {
            name,
            bytes: meta.len(),
            modified,
        });
    }
    let doomed = evict_plan(&entries, cap);
    for name in &doomed {
        fs::remove_file(dir.join(name))?;
    }
    Ok(doomed.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;

    /// Answers from a script, in order; counts the calls.
    struct Script {
        replies: RefCell<VecDeque<Result<Vec<u8>, HttpError>>>,
        calls: Cell<u32>,
    }

    impl Script {
        fn new(replies: Vec<Result<Vec<u8>, HttpError>>) -> Script {
            Script {
                replies: RefCell::new(replies.into()),
                calls: Cell::new(0),
            }
        }
    }

    impl TileClient for Script {
        fn get(&self, _url: &str) -> Result<Vec<u8>, HttpError> {
            self.calls.set(self.calls.get() + 1);
            self.replies
                .borrow_mut()
                .pop_front()
                .expect("unscripted call")
        }
    }

    fn status(code: u16) -> Result<Vec<u8>, HttpError> {
        Err(HttpError {
            status: Some(code),
            message: format!("HTTP {code}"),
        })
    }

    fn offline() -> Result<Vec<u8>, HttpError> {
        Err(HttpError {
            status: None,
            message: "no route".into(),
        })
    }

    const TPL: &str = "https://tiles.openfreemap.org/planet/20261101_080013_pt/{z}/{x}/{y}.pbf";
    const ID: TileId = TileId {
        z: 14,
        x: 6192,
        y: 9115,
    };

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("worldmap-fetch-test-{name}"));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn no_sleep(_: Duration) {}

    #[test]
    fn template_is_read_from_tilejson_and_versioned() {
        let body = r#"{"tilejson":"3.0.0","tiles":["https://tiles.openfreemap.org/planet/20261101_080013_pt/{z}/{x}/{y}.pbf"]}"#;
        assert_eq!(template_from_tilejson(body).unwrap(), TPL);
        assert_eq!(version_of(TPL), "20261101_080013_pt");
        assert!(template_from_tilejson(r#"{"tiles":["http://x/{z}/{x}/{y}.pbf"]}"#).is_err());
        assert!(template_from_tilejson(r#"{"tiles":[]}"#).is_err());
    }

    #[test]
    fn url_and_file_name_follow_z_x_y_and_version() {
        assert_eq!(
            tile_url(TPL, ID),
            "https://tiles.openfreemap.org/planet/20261101_080013_pt/14/6192/9115.pbf"
        );
        assert_eq!(
            cache_file_name(TPL, ID),
            "20261101_080013_pt_14_6192_9115.pbf"
        );
    }

    #[test]
    fn cached_tile_is_not_downloaded() {
        let d = dir("hit");
        fs::create_dir_all(&d).unwrap();
        fs::write(cache_path(&d, TPL, ID), b"tile").unwrap();
        let client = Script::new(vec![]);
        let (bytes, source) = load_tile(&d, TPL, ID, &client, &no_sleep).unwrap();
        assert_eq!((bytes.as_slice(), source), (&b"tile"[..], Source::Cache));
        assert_eq!(client.calls.get(), 0);
    }

    #[test]
    fn download_is_written_then_read_from_cache() {
        let d = dir("miss");
        let client = Script::new(vec![Ok(b"tile".to_vec())]);
        let (_, source) = load_tile(&d, TPL, ID, &client, &no_sleep).unwrap();
        assert_eq!(source, Source::Network);
        let (bytes, source) = load_tile(&d, TPL, ID, &client, &no_sleep).unwrap();
        assert_eq!((bytes.as_slice(), source), (&b"tile"[..], Source::Cache));
        assert_eq!(client.calls.get(), 1);
    }

    #[test]
    fn rate_limit_and_server_errors_are_retried_with_doubling_waits() {
        let d = dir("retry");
        let client = Script::new(vec![status(429), status(503), Ok(b"ok".to_vec())]);
        let waits = RefCell::new(Vec::new());
        let sleep = |w: Duration| waits.borrow_mut().push(w);
        let (bytes, _) = load_tile(&d, TPL, ID, &client, &sleep).unwrap();
        assert_eq!(bytes, b"ok");
        assert_eq!(client.calls.get(), 3);
        assert_eq!(*waits.borrow(), vec![BACKOFF_BASE, BACKOFF_BASE * 2]);
    }

    #[test]
    fn not_found_fails_at_once_and_leaves_no_file() {
        let d = dir("404");
        let client = Script::new(vec![status(404)]);
        assert!(load_tile(&d, TPL, ID, &client, &no_sleep).is_err());
        assert_eq!(client.calls.get(), 1);
        assert!(!cache_path(&d, TPL, ID).exists());
    }

    #[test]
    fn gives_up_after_max_attempts_when_offline() {
        let d = dir("offline");
        let client = Script::new((0..MAX_ATTEMPTS).map(|_| offline()).collect());
        assert!(load_tile(&d, TPL, ID, &client, &no_sleep).is_err());
        assert_eq!(client.calls.get(), MAX_ATTEMPTS);
    }

    #[test]
    fn template_falls_back_to_the_saved_one_when_offline() {
        let d = dir("template");
        let online = Script::new(vec![Ok(format!(r#"{{"tiles":["{TPL}"]}}"#).into_bytes())]);
        assert_eq!(load_template(&d, &online, &no_sleep).unwrap(), TPL);
        let offline_client = Script::new((0..MAX_ATTEMPTS).map(|_| offline()).collect());
        assert_eq!(load_template(&d, &offline_client, &no_sleep).unwrap(), TPL);
        let empty = dir("template-empty");
        assert!(load_template(
            &empty,
            &Script::new((0..MAX_ATTEMPTS).map(|_| offline()).collect()),
            &no_sleep
        )
        .is_err());
    }

    #[test]
    fn eviction_drops_the_oldest_until_under_cap() {
        let e = |name: &str, bytes, modified| Entry {
            name: name.into(),
            bytes,
            modified,
        };
        let entries = vec![e("c.pbf", 40, 3), e("a.pbf", 40, 1), e("b.pbf", 40, 2)];
        assert_eq!(evict_plan(&entries, 100), vec!["a.pbf".to_string()]);
        assert!(evict_plan(&entries, 120).is_empty());
        assert_eq!(evict_plan(&entries, 0).len(), 3);
    }
}
