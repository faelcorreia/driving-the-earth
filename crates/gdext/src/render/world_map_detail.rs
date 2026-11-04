//! Background loader of OpenFreeMap vector tiles for the home picker (F17b3). Three threads take
//! tiles from one queue, nearest first. The main thread replaces the queue each time the view moves,
//! so tiles not started yet that left the screen are dropped. A download already in flight finishes
//! and is cached. Results come back through a channel and are drawn when they arrive. The logic is
//! in `worldmap::fetch` and `worldmap::tiles`; this file only owns the threads.
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

use worldmap::fetch::{
    cache_path, enforce_cap, load_template, load_tile, Source, UreqTiles, CACHE_CAP_BYTES,
};
use worldmap::tiles::{decode, TileData, TileId};

/// After a failed tile (or template), it is not asked again for this long (offline is not retried
/// every frame).
pub const RETRY_AFTER: Duration = Duration::from_secs(60);
const IDLE: Duration = Duration::from_millis(100);
/// Download threads. Requests in flight at once stay within this.
pub const WORKERS: usize = 3;

pub type Done = (TileId, Result<TileData, String>);

#[derive(Default)]
struct Queue {
    tiles: VecDeque<TileId>,
}

/// The tile URL template, asked once per session (and again after `RETRY_AFTER` if it failed).
#[derive(Default)]
struct Template {
    value: Option<String>,
    tried: Option<Instant>,
}

pub struct Loader {
    queue: Arc<Mutex<Queue>>,
    results: mpsc::Receiver<Done>,
}

impl Loader {
    /// Starts the threads. They stop by themselves once the loader is dropped.
    pub fn start(dir: PathBuf) -> Loader {
        let queue = Arc::new(Mutex::new(Queue::default()));
        let template = Arc::new(Mutex::new(Template::default()));
        let (tx, rx) = mpsc::channel();
        let mut started = 0;
        for i in 0..WORKERS {
            let (shared, template, dir, tx) = (
                Arc::downgrade(&queue),
                template.clone(),
                dir.clone(),
                tx.clone(),
            );
            let spawned = thread::Builder::new()
                .name(format!("worldmap-tiles-{i}"))
                .spawn(move || work(shared, template, dir, tx));
            if spawned.is_ok() {
                started += 1;
            }
        }
        // Without any thread the picker keeps the embedded land and cities only.
        let results = if started > 0 { rx } else { mpsc::channel().1 };
        Loader { queue, results }
    }

    /// Replaces the wanted list (nearest first).
    pub fn want(&self, tiles: Vec<TileId>) {
        if let Ok(mut q) = self.queue.lock() {
            q.tiles = tiles.into();
        }
    }

    pub fn drain(&self) -> Vec<Done> {
        self.results.try_iter().collect()
    }
}

fn work(
    shared: Weak<Mutex<Queue>>,
    template: Arc<Mutex<Template>>,
    dir: PathBuf,
    tx: mpsc::Sender<Done>,
) {
    loop {
        let Some(queue) = shared.upgrade() else {
            break;
        };
        let next = queue.lock().ok().and_then(|mut q| q.tiles.pop_front());
        drop(queue);
        let Some(id) = next else {
            thread::sleep(IDLE);
            continue;
        };
        let result = fetch_one(&template, &dir, id);
        if tx.send((id, result)).is_err() {
            break;
        }
    }
}

fn fetch_one(template: &Mutex<Template>, dir: &Path, id: TileId) -> Result<TileData, String> {
    let url = template_for(template, dir)?;
    let (bytes, source) = load_tile(dir, &url, id, &UreqTiles, &|d| thread::sleep(d))?;
    if source == Source::Network {
        let _ = enforce_cap(dir, CACHE_CAP_BYTES);
    }
    match decode(id, &bytes) {
        Ok(data) => Ok(data),
        Err(e) => {
            // A corrupt cache file is removed, so the next try downloads it again.
            if source == Source::Cache {
                let _ = std::fs::remove_file(cache_path(dir, &url, id));
            }
            Err(e)
        }
    }
}

/// The template, asked for by the first worker that needs it. The lock is held while asking, so
/// the others wait here (their own thread, not the picker's).
fn template_for(state: &Mutex<Template>, dir: &Path) -> Result<String, String> {
    let mut t = state.lock().map_err(|_| "template lock".to_string())?;
    if let Some(value) = &t.value {
        return Ok(value.clone());
    }
    if t.tried.is_some_and(|at| at.elapsed() < RETRY_AFTER) {
        return Err("tile template unavailable".into());
    }
    t.tried = Some(Instant::now());
    let asked = load_template(dir, &UreqTiles, &|d| thread::sleep(d));
    if let Ok(value) = &asked {
        t.value = Some(value.clone());
    }
    asked
}
