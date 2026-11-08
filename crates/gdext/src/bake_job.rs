//! Background bake exposed to the game as the `BakeJob` node (item 76).
//!
//! `BakeSlot` is the plain part: one bake at a time on its own thread, progress and results
//! sent back as `BakeEvent`s, cancellation by a flag that `tilebake` checks between blocks and
//! tiles. `BakeJob` is the node: `process()` drains the events and emits the signals, so the
//! main thread never waits for a bake. The cache is the same folder the game reads from
//! (`user://cache` by default), and a step already on disk is skipped by `tilebake`.
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use godot::classes::ProjectSettings;
use godot::prelude::*;
use tilebake::Progress;

/// Default cache root: the user's data folder, as in `EarthWorld`.
const DEFAULT_CACHE_ROOT: &str = "user://cache";

/// `kind` of the failure that ends a bake the player cancelled.
pub const KIND_CANCELLED: &str = "cancelled";

/// What a bake reports, in order: any number of `Progress`, then one `Finished` or `Failed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BakeEvent {
    Progress {
        stage: &'static str,
        done: usize,
        total: usize,
    },
    Finished {
        map_id: String,
    },
    Failed {
        kind: &'static str,
        message: String,
    },
}

/// Kind of a failed bake: `cancelled` for an interrupted run, `invalid_input` for a bad request,
/// `other` for the rest (disk, network, data).
pub fn failure_kind(e: &io::Error) -> &'static str {
    match e.kind() {
        io::ErrorKind::Interrupted => KIND_CANCELLED,
        io::ErrorKind::InvalidInput => "invalid_input",
        _ => "other",
    }
}

/// One bake at a time: a thread, its events, and its cancel flag. Dropping the slot cancels a
/// running bake and waits for its thread to end.
#[derive(Default)]
pub struct BakeSlot {
    thread: Option<JoinHandle<()>>,
    events: Option<Receiver<BakeEvent>>,
    cancel: Option<Arc<AtomicBool>>,
}

impl BakeSlot {
    /// True from `start` until the terminal event is drained by `poll`.
    pub fn is_running(&self) -> bool {
        self.thread.is_some()
    }

    /// Runs `bake` on a new thread, with a cancel flag and a progress callback. Returns false,
    /// and starts nothing, while a bake of this slot is still running.
    pub fn start<F>(&mut self, bake: F) -> bool
    where
        F: FnOnce(&AtomicBool, &mut dyn FnMut(Progress)) -> io::Result<String> + Send + 'static,
    {
        if self.is_running() {
            return false;
        }
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let thread = thread::spawn(move || {
            let mut progress = |p: Progress| {
                let _ = tx.send(BakeEvent::Progress {
                    stage: p.stage,
                    done: p.done,
                    total: p.total,
                });
            };
            let terminal = match bake(&flag, &mut progress) {
                Ok(map_id) => BakeEvent::Finished { map_id },
                Err(e) => BakeEvent::Failed {
                    kind: failure_kind(&e),
                    message: e.to_string(),
                },
            };
            let _ = tx.send(terminal);
        });
        self.thread = Some(thread);
        self.events = Some(rx);
        self.cancel = Some(cancel);
        true
    }

    /// Asks the running bake to stop. It stops at its next check (between blocks and tiles), and
    /// the terminal `Failed` with kind `cancelled` comes from `poll`. Does nothing when idle.
    pub fn cancel(&self) {
        if let Some(flag) = &self.cancel {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Events since the last call, without waiting. Consecutive `Progress` events of the same
    /// stage are merged into the last one. When the terminal event comes, the thread is joined
    /// and the slot is free again. A thread that ends without a terminal event (a panic) gives a
    /// `Failed` with kind `other`.
    pub fn poll(&mut self) -> Vec<BakeEvent> {
        let mut out: Vec<BakeEvent> = Vec::new();
        let Some(rx) = &self.events else {
            return out;
        };
        let mut ended = false;
        loop {
            match rx.try_recv() {
                Ok(event) => {
                    let terminal = !matches!(event, BakeEvent::Progress { .. });
                    push_merged(&mut out, event);
                    if terminal {
                        ended = true;
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    out.push(BakeEvent::Failed {
                        kind: "other",
                        message: "bake thread ended without a result".into(),
                    });
                    ended = true;
                    break;
                }
            }
        }
        if ended {
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            self.events = None;
            self.cancel = None;
        }
        out
    }
}

impl Drop for BakeSlot {
    fn drop(&mut self) {
        self.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Appends `event`, replacing the last entry when both are `Progress` of the same stage.
fn push_merged(out: &mut Vec<BakeEvent>, event: BakeEvent) {
    if let (Some(BakeEvent::Progress { stage: last, .. }), BakeEvent::Progress { stage, .. }) =
        (out.last(), &event)
    {
        if last == stage {
            out.pop();
        }
    }
    out.push(event);
}

/// Godot node that bakes a point in the background and reports it with signals.
#[derive(GodotClass)]
#[class(base = Node, init)]
pub struct BakeJob {
    base: Base<Node>,
    /// Cache root, a `user://` (or `res://`) path. The game uses `user://cache`.
    #[init(val = GString::from(DEFAULT_CACHE_ROOT))]
    #[export]
    cache_root: GString,
    slot: BakeSlot,
}

#[godot_api]
impl INode for BakeJob {
    fn ready(&mut self) {
        self.base_mut().set_process(true);
    }

    fn process(&mut self, _delta: f64) {
        self.drain();
    }
}

#[godot_api]
impl BakeJob {
    /// Stage progress: `stage` is the stage key (`osm`, `osm_areas`, `dem`, `tiles`, `roads`,
    /// `far_lights`), `done` of `total` units.
    #[signal]
    fn progress(stage: GString, done: i64, total: i64);

    /// The bake is complete; `map_id` is the folder under the cache root.
    #[signal]
    fn finished(map_id: GString);

    /// The bake stopped. `kind` is `cancelled`, `invalid_input` or `other`.
    #[signal]
    fn failed(kind: GString, message: GString);

    /// Starts baking the point (`lat`, `lon`, in degrees) with radius `radius_m` (meters). The id
    /// is `tilebake::id_for_coords`. Returns false, and starts nothing, while this node is baking.
    /// Errors (bad input, network, disk) come later as `failed`.
    #[func]
    fn start(&mut self, lat: f64, lon: f64, radius_m: f64) -> bool {
        let root = PathBuf::from(
            ProjectSettings::singleton()
                .globalize_path(&self.cache_root)
                .to_string(),
        );
        let id = tilebake::id_for_coords(lat, lon);
        let name = id.clone();
        let started = self.slot.start(move |cancel, progress| {
            tilebake::bake_region(&root, &id, &name, lat, lon, radius_m, cancel, progress)
                .map(|r| r.osm.meta.id)
        });
        if started {
            godot_print!("bake: start lat {lat:.6} lon {lon:.6} radius {radius_m} m");
        }
        started
    }

    /// Asks the running bake to stop. The `failed` signal with kind `cancelled` follows. The folder
    /// keeps what was finished, so the next `start` of the same point skips it.
    #[func]
    fn cancel(&mut self) {
        if self.slot.is_running() {
            godot_print!("bake: cancel requested");
        }
        self.slot.cancel();
    }

    /// True from `start` until the bake ends (its signal has been emitted).
    #[func]
    fn is_running(&self) -> bool {
        self.slot.is_running()
    }
}

impl BakeJob {
    /// Emits the signals for the events the slot has since the last frame.
    fn drain(&mut self) {
        for event in self.slot.poll() {
            match event {
                BakeEvent::Progress { stage, done, total } => {
                    let stage = GString::from(stage);
                    self.signals()
                        .progress()
                        .emit(&stage, done as i64, total as i64);
                }
                BakeEvent::Finished { map_id } => {
                    godot_print!("bake: finished {map_id}");
                    let map_id = GString::from(map_id.as_str());
                    self.signals().finished().emit(&map_id);
                }
                BakeEvent::Failed { kind, message } => {
                    if kind == KIND_CANCELLED {
                        godot_print!("bake: cancelled");
                    } else {
                        godot_error!("bake: failed ({kind}): {message}");
                    }
                    let kind = GString::from(kind);
                    let message = GString::from(message.as_str());
                    self.signals().failed().emit(&kind, &message);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Polls `slot` until an event matches `want`, or fails after 10 s. Returns all events seen.
    fn wait_for(slot: &mut BakeSlot, want: impl Fn(&BakeEvent) -> bool) -> Vec<BakeEvent> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        loop {
            seen.extend(slot.poll());
            if seen.iter().any(&want) {
                return seen;
            }
            assert!(Instant::now() < deadline, "timeout, events so far: {seen:?}");
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Simulated bake of `steps` steps: each step waits for a token on `gate`, then checks the
    /// cancel flag the way `tilebake` does between blocks and tiles, then reports progress.
    fn gated_bake(
        steps: usize,
        gate: Receiver<()>,
    ) -> impl FnOnce(&AtomicBool, &mut dyn FnMut(Progress)) -> io::Result<String> + Send + 'static
    {
        move |cancel, progress| {
            for done in 0..steps {
                gate.recv()
                    .map_err(|_| io::Error::new(io::ErrorKind::Other, "gate closed"))?;
                if cancel.load(Ordering::Relaxed) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "bake cancelled"));
                }
                progress(Progress {
                    stage: "tiles",
                    done: done + 1,
                    total: steps,
                });
            }
            Ok("sim".into())
        }
    }

    #[test]
    fn only_one_bake_runs_at_a_time() {
        let mut slot = BakeSlot::default();
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        assert!(slot.start(gated_bake(1, gate_rx)));
        assert!(slot.is_running());
        // The second start is refused while the first runs, and it starts nothing.
        assert!(!slot.start(|_, _| Ok("second".into())));

        gate_tx.send(()).unwrap();
        let events = wait_for(&mut slot, |e| matches!(e, BakeEvent::Finished { .. }));
        assert!(events.contains(&BakeEvent::Finished { map_id: "sim".into() }));
        assert!(!slot.is_running());

        // Free again after the terminal event.
        assert!(slot.start(|_, _| Ok("third".into())));
        wait_for(&mut slot, |e| matches!(e, BakeEvent::Finished { .. }));
    }

    #[test]
    fn cancel_stops_between_steps_with_kind_cancelled() {
        let mut slot = BakeSlot::default();
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        assert!(slot.start(gated_bake(10, gate_rx)));

        gate_tx.send(()).unwrap();
        gate_tx.send(()).unwrap();
        wait_for(&mut slot, |e| {
            matches!(e, BakeEvent::Progress { done: 2, .. })
        });

        // Cancel before the third step: the step's check sees the flag and the bake stops.
        slot.cancel();
        gate_tx.send(()).unwrap();
        let events = wait_for(&mut slot, |e| matches!(e, BakeEvent::Failed { .. }));
        assert_eq!(
            events.last(),
            Some(&BakeEvent::Failed {
                kind: KIND_CANCELLED,
                message: "bake cancelled".into(),
            })
        );
        assert!(!events.iter().any(|e| matches!(e, BakeEvent::Finished { .. })));
        assert!(!events.iter().any(|e| matches!(e, BakeEvent::Progress { done: 3, .. })));
        assert!(!slot.is_running());

        // The slot takes a new bake after a cancel, with a fresh flag (not cancelled).
        assert!(slot.start(|_, _| Ok("again".into())));
        wait_for(&mut slot, |e| matches!(e, BakeEvent::Finished { .. }));
    }

    #[test]
    fn cancel_when_idle_does_nothing_and_a_panic_is_reported() {
        let mut slot = BakeSlot::default();
        slot.cancel();
        assert!(slot.poll().is_empty());

        assert!(slot.start(|_, _| panic!("simulated panic")));
        let events = wait_for(&mut slot, |e| matches!(e, BakeEvent::Failed { .. }));
        assert!(matches!(
            events.last(),
            Some(BakeEvent::Failed { kind: "other", .. })
        ));
        assert!(!slot.is_running());
    }

    #[test]
    fn dropping_a_running_slot_cancels_and_joins() {
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let mut slot = BakeSlot::default();
        assert!(slot.start(gated_bake(5, gate_rx)));
        // Enough tokens for every step. Drop cancels and joins: the test returns only if the thread ends.
        for _ in 0..5 {
            gate_tx.send(()).unwrap();
        }
        drop(slot);
    }

    #[test]
    fn failure_kinds_follow_io_error_kinds() {
        let cancelled = io::Error::new(io::ErrorKind::Interrupted, "x");
        let invalid = io::Error::new(io::ErrorKind::InvalidInput, "x");
        let other = io::Error::new(io::ErrorKind::Other, "x");
        assert_eq!(failure_kind(&cancelled), KIND_CANCELLED);
        assert_eq!(failure_kind(&invalid), "invalid_input");
        assert_eq!(failure_kind(&other), "other");
    }
}
