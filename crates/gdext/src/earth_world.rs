//! Godot node that streams a baked map around the car. The map is opened without reading
//! its tiles; the car is spawned on the road nearest to home (the map center), facing south.
//! The opening runs in real stages across frames (see `LoadingStage`) behind `loading.tscn`:
//! map, spawn search, one spawn tile per frame, car, environment and stream. Then the tiles
//! within `view_radius_m` of the car are built in the background and instantiated on the main
//! thread, a few per frame, and tiles that fall beyond the radius plus a margin are freed.
use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use godot::classes::file_access::ModeFlags;
use godot::classes::{
    CanvasLayer, FileAccess, INode3D, Material, MeshInstance3D, Node3D, PackedScene,
    ProjectSettings, RigidBody3D, ShaderMaterial, Os,
};
use godot::prelude::*;
use worldmap::home::Home;

use crate::environment::lamps::{delay_s, is_lit};
use crate::environment::EnvironmentClock;
use crate::geometry::lamps::LampPost;
use crate::geometry::{mesh_cell_for, TileGeometry};
use crate::home_store;
use crate::render::far_lights::{rss_mb, DistantLights};
use crate::render::lamps::{head_position, lamp_node, LampLights, LampMaterials, LAMP_POOL};
use crate::render::world_map::WorldMapView;
use crate::render::{
    building_body, building_material, road_material, solid_body, spawn_car, terrain_body,
    terrain_material, vertex_material,
};
use crate::style;
use crate::ui_text::{language, text as ui_text, Lang};
use crate::world::far_lights::{spawn_load, FarLoad};
use crate::world::lod::{self, TerrainLod};
use crate::world::spawn::{find_spawn, find_spawn_near, inside_loaded_map, SpawnPose};
use crate::world::streaming::{distance_to_tile, is_stale, tiles_in_range};
use crate::world::{open_map, MapIndex};
use tilebake::dem::MapGrid;
use tileformat::units::lamps::LAMP_REAL_REACH_M;

/// Height of the car above the terrain when spawned, in meters.
const SPAWN_LIFT_M: f32 = 1.0;
/// Tiles instantiated (nodes, meshes and collision shapes) on the main thread per frame.
const TILES_PER_FRAME: usize = 2;
/// Geometry requests the loader thread may have outstanding.
const MAX_IN_FLIGHT: usize = 8;
/// A loaded tile is freed only beyond the view radius plus this, in meters.
const HYSTERESIS_M: f64 = 150.0;
/// Period of the timing line in the log, in seconds.
const STATS_PERIOD_S: f64 = 5.0;
/// Canvas layer of the 2D map, above the 3D view and the HUD.
const MAP_CANVAS_LAYER: i32 = 10;
/// Period of the real lights update (which lit lamps they stand on), in seconds.
const LAMP_UPDATE_S: f64 = 0.25;
/// Default cache root: the user's data folder, in every build.
const DEFAULT_CACHE_ROOT: &str = "user://cache";
/// Repo cache, used in development when the default cache root is kept and this folder exists.
const DEV_CACHE_ROOT: &str = "res://../cache";
/// Map opened in development when there is no `map_id` and no last map yet.
const DEV_DEFAULT_MAP: &str = "casa-10km";
/// The last map opened, as its id in a text file.
const LAST_MAP_PATH: &str = "user://last_map.txt";
/// Canvas layer of the loading screen, above the 3D view and below the world map.
const LOADING_CANVAS_LAYER: i32 = 15;

/// Real steps of the opening, one per frame (except the tiles, one per frame each). The loading
/// screen is added and drawn before the first one; the car exists only after `PlacingCar`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum LoadingStage {
    /// `open_map`: read the map metadata and register the map as the last one.
    #[default]
    OpeningMap,
    /// Saved home, spawn search, materials and the list of spawn tiles.
    ReadingMap,
    /// The spawn tiles, one `load_now` per frame.
    BuildingTiles,
    /// `spawn_car`.
    PlacingCar,
    /// `Environment` clock and the pool of real lamp lights.
    Environment,
    /// Loader thread, distant lights and terrain LOD; the car enters the tree here.
    StartingStream,
    /// The world map view, whose world data load is the heaviest step left.
    WorldMap,
    /// Screen closed; the normal streaming `process` runs.
    Ready,
}

/// Stage after `stage`. A pure function, so the order is unit-tested without Godot.
fn next_stage(stage: LoadingStage) -> LoadingStage {
    match stage {
        LoadingStage::OpeningMap => LoadingStage::ReadingMap,
        LoadingStage::ReadingMap => LoadingStage::BuildingTiles,
        LoadingStage::BuildingTiles => LoadingStage::PlacingCar,
        LoadingStage::PlacingCar => LoadingStage::Environment,
        LoadingStage::Environment => LoadingStage::StartingStream,
        LoadingStage::StartingStream => LoadingStage::WorldMap,
        LoadingStage::WorldMap | LoadingStage::Ready => LoadingStage::Ready,
    }
}

/// Progress (0..1) of `stage`. `built` and `total` only matter while building the spawn tiles.
fn stage_progress(stage: LoadingStage, built: usize, total: usize) -> f64 {
    match stage {
        LoadingStage::OpeningMap => 0.02,
        LoadingStage::ReadingMap => 0.12,
        LoadingStage::BuildingTiles => {
            let done = built.min(total.max(1)) as f64 / total.max(1) as f64;
            0.20 + 0.36 * done
        }
        LoadingStage::PlacingCar => 0.60,
        LoadingStage::Environment => 0.72,
        LoadingStage::StartingStream => 0.86,
        LoadingStage::WorldMap => 0.94,
        LoadingStage::Ready => 1.0,
    }
}

/// Line of the loading screen for `stage`.
fn stage_key(stage: LoadingStage) -> &'static str {
    match stage {
        LoadingStage::OpeningMap => "loading.opening_map",
        LoadingStage::ReadingMap => "loading.reading_map",
        LoadingStage::BuildingTiles => "loading.building_tiles",
        LoadingStage::PlacingCar => "loading.placing_car",
        LoadingStage::Environment => "loading.environment",
        LoadingStage::StartingStream | LoadingStage::Ready => "loading.ready",
        LoadingStage::WorldMap => "loading.world_map",
    }
}

/// Stage name for the log.
fn stage_label(stage: LoadingStage) -> &'static str {
    match stage {
        LoadingStage::OpeningMap => "opening map",
        LoadingStage::ReadingMap => "reading map",
        LoadingStage::BuildingTiles => "building tiles",
        LoadingStage::PlacingCar => "placing car",
        LoadingStage::Environment => "environment",
        LoadingStage::StartingStream => "starting stream",
        LoadingStage::WorldMap => "world map",
        LoadingStage::Ready => "ready",
    }
}

/// Streams `<cache_root>/<map_id>` around the car. With no map (no `map_id`, no last map, folder
/// missing) it emits `no_map` and opens the world map with its message; no error is logged.
#[derive(GodotClass)]
#[class(base = Node3D, init)]
pub struct EarthWorld {
    base: Base<Node3D>,
    /// Folder name of the map under the cache root (the id given to tilebake). Empty: the last
    /// map opened, or in development the repo's default map.
    #[init(val = GString::new())]
    #[export]
    map_id: GString,
    /// Visual theme id (`godot/styles/<id>.json`). Empty: `--style <id>` from the command line,
    /// else `lowpoly`.
    #[init(val = GString::new())]
    #[export]
    style_id: GString,
    /// Cache root. `user://cache` by default; in development `res://../cache` is used instead
    /// when it exists. `res://` paths are resolved to the file system.
    #[init(val = GString::from(DEFAULT_CACHE_ROOT))]
    #[export]
    cache_root: GString,
    /// Tiles within this distance of the car are loaded, in meters.
    #[init(val = 1000.0)]
    #[export]
    view_radius_m: f32,
    /// Real lights on the lit lamps nearest the car: 0 turns them off, at most `LAMP_POOL`.
    #[init(val = 4)]
    #[export]
    lamp_real_lights: i64,
    /// Light pools on the ground under the lit lamps.
    #[init(val = true)]
    #[export]
    lamp_glow_enabled: bool,
    /// Distant lights from `far_lights.bin` of the map (loaded in the background, none when the
    /// file is missing).
    #[init(val = true)]
    #[export]
    far_lights_enabled: bool,
    /// Most distant lights kept, the heaviest by weight; the rest are cut.
    #[init(val = 150000)]
    #[export]
    far_lights_max: i64,

    index: Option<Arc<MapIndex>>,
    car: Option<Gd<Node3D>>,
    /// Materials of the low-poly toon light in `lowpoly`, plain standard materials otherwise
    /// (backlog 125): the same single surface either way, so no extra draw call.
    terrain_mat: Option<Gd<Material>>,
    road_mat: Option<Gd<Material>>,
    /// White albedo: meshes with per-vertex colors (buildings, terrain with ground cover) show
    /// their colors as they are.
    vertex_mat: Option<Gd<Material>>,
    /// Material of the buildings: vertex colors, windows by mask (None: `vertex_mat` is used).
    building_mat: Option<Gd<ShaderMaterial>>,
    /// Loaded tiles, each one a container node with the terrain and road bodies.
    loaded: HashMap<(i32, i32), Gd<Node3D>>,
    /// Side of the drawn terrain grid each loaded tile was built with (see `mesh_cell_for`).
    built_cell: HashMap<(i32, i32), f64>,
    /// Geometry requested from the loader and not back yet.
    requested: HashSet<(i32, i32)>,
    /// Tiles whose geometry failed to load; not retried.
    failed: HashSet<(i32, i32)>,
    /// Geometry back from the loader, waiting for its turn to be instantiated.
    done: VecDeque<((i32, i32), TileGeometry)>,
    /// Tile and the side of its drawn terrain grid to build.
    requests: Option<Sender<((i32, i32), f64)>>,
    results: Option<Receiver<((i32, i32), io::Result<TileGeometry>)>>,
    /// Distant terrain rings, and the node of the last build.
    lod: Option<TerrainLod>,
    lod_node: Option<Gd<MeshInstance3D>>,

    lamp_mats: Option<LampMaterials>,
    lamp_lights: Option<LampLights>,
    /// Lamp posts of each loaded tile, for the real lights.
    lamp_posts: HashMap<(i32, i32), Vec<LampPost>>,
    clock: Option<Gd<EnvironmentClock>>,
    lamp_timer: f64,
    /// Lit lamps (all loaded tiles) and lamps in total, at the last real-light update.
    lamps_lit: usize,
    lamps_total: usize,

    /// Distant lights: the background load, then the node once built.
    far_loader: Option<Receiver<io::Result<FarLoad>>>,
    far: Option<DistantLights>,

    /// Loading screen (item 123), the stage it is on and the timers of the stage and of the whole
    /// opening. `ready` only adds the screen; `process` runs one stage per frame.
    loading: Option<Gd<Node>>,
    stage: LoadingStage,
    loading_started: Option<Instant>,
    stage_started: Option<Instant>,
    /// First `process` only draws the screen: the expensive stages start on the next frame.
    first_frame_pending: bool,
    /// `--loading-hold <seconds>`: debug hold of the last stage, 0 disables it.
    loading_hold_s: f64,
    loading_hold_until: Option<Instant>,
    lang: Option<Lang>,
    /// Map state kept between the staged steps: index and cache opened, map id, spawn pose and the
    /// spawn tiles still to build.
    pending_index: Option<Arc<MapIndex>>,
    pending_cache: Option<PathBuf>,
    pending_map_id: String,
    pending_picker_key: Option<String>,
    pending_car_position: Option<Vector3>,
    pending_car_yaw: f32,
    pending_tiles: VecDeque<(i32, i32)>,
    pending_built: usize,
    pending_total: usize,

    /// Cache root and id of the map in use, kept for a switch to another baked map (item 106).
    active_cache: Option<PathBuf>,
    active_id: String,
    /// Grid of the map in use. Shared with the world map view, which reads it to place the car on
    /// its picture; a switch of map sets it again.
    map_grid: Rc<Cell<Option<MapGrid>>>,

    /// Worst `process` call and worst tile instantiation since the last log line, seconds.
    worst_frame: f64,
    worst_build: f64,
    stats_timer: f64,
}

#[godot_api]
impl EarthWorld {
    /// Emitted when no map can be opened: no map requested, or its folder is missing.
    #[signal]
    fn no_map();

    /// Puts the car on the road nearest to (`lat`, `lon`) when that point is inside the loaded
    /// map (F17c). The car is stopped and its tiles are built before it arrives; the streaming
    /// then follows it. Returns false when the point is outside the map or has no road, and
    /// the car stays where it was.
    #[func]
    fn move_car_to_latlon(&mut self, lat: f64, lon: f64) -> bool {
        let (inside, road, result) = self.move_car_checked(lat, lon);
        godot_print!(
            "confirm move: lat {lat:.6} lon {lon:.6} inside_loaded_map {inside} road_found {road} result {result}"
        );
        result == "moved"
    }

    /// Puts the car at (`lat`, `lon`) (item 106). Inside the loaded map it is the same move as
    /// `move_car_to_latlon`. Outside it, the first other complete map of the cache that holds the
    /// point and has a road there replaces the loaded map, and the car goes to that road. False when
    /// no baked map holds a road at the point: the car stays where it was.
    #[func]
    fn travel_to_latlon(&mut self, lat: f64, lon: f64) -> bool {
        let (inside, road, result) = self.move_car_checked(lat, lon);
        if inside {
            godot_print!(
                "confirm move: lat {lat:.6} lon {lon:.6} inside_loaded_map {inside} road_found {road} result {result}"
            );
            return result == "moved";
        }
        let (Some(cache), Some(_)) = (self.active_cache.clone(), self.car.clone()) else {
            godot_print!("confirm move: lat {lat:.6} lon {lon:.6} no map or car to switch from");
            return false;
        };
        let Some((id, index, pose)) = self.find_other_map(&cache, lat, lon) else {
            godot_print!("confirm move: lat {lat:.6} lon {lon:.6} inside_loaded_map false, no other baked map holds a road there");
            return false;
        };
        godot_print!("confirm move: lat {lat:.6} lon {lon:.6} switching the loaded map to '{id}'");
        self.switch_map(cache, id, index, pose)
    }

    /// The steps of a car move, without the log line: (inside the loaded map, road found, result).
    fn move_car_checked(&mut self, lat: f64, lon: f64) -> (bool, bool, String) {
        let (Some(index), Some(mut car)) = (self.index.clone(), self.car.clone()) else {
            return (false, false, "no_map_or_car".into());
        };
        let tile_m = index.meta.tile_size_m;
        let grid = MapGrid::from_meta(&index.meta);
        let Some(local) = inside_loaded_map(&grid, index.meta.radius_m, lat, lon, |t| {
            index.has_tile(t.0, t.1)
        }) else {
            return (false, false, "outside_loaded_map".into());
        };
        let pose = match find_spawn_near(&index, local, self.view_radius_m as f64) {
            Ok(Some(pose)) => pose,
            Ok(None) => return (true, false, "no_road".into()),
            Err(e) => {
                godot_error!("cannot read map for the car move: {e}");
                return (true, false, format!("error_map: {e}"));
            }
        };
        let spawn_tile = (
            (pose.x / tile_m).floor() as i32,
            (pose.y / tile_m).floor() as i32,
        );
        let near = match index.neighborhood(spawn_tile.0, spawn_tile.1) {
            Ok(map) => map,
            Err(e) => {
                godot_error!("cannot read tiles at the new road: {e}");
                return (true, true, format!("error_tiles: {e}"));
            }
        };
        let Some(height) = near.height_at(pose.x, pose.y) else {
            return (true, true, "no_terrain".into());
        };
        for ty in spawn_tile.1 - 1..=spawn_tile.1 + 1 {
            for tx in spawn_tile.0 - 1..=spawn_tile.0 + 1 {
                self.load_now(&index, (tx, ty));
            }
        }
        let position = Vector3::new(pose.x as f32, height + SPAWN_LIFT_M, -(pose.y as f32));
        car.set_position(position);
        car.set_rotation(Vector3::new(0.0, pose.yaw, 0.0));
        if let Ok(mut body) = car.clone().try_cast::<RigidBody3D>() {
            body.set_linear_velocity(Vector3::ZERO);
            body.set_angular_velocity(Vector3::ZERO);
        }
        (true, true, "moved".into())
    }
}

/// The id of the last map opened, from `LAST_MAP_PATH`. None when there is no file yet.
fn read_last_map() -> Option<String> {
    let file = FileAccess::open(LAST_MAP_PATH, ModeFlags::READ)?;
    let id = file.get_as_text().to_string();
    let id = id.trim();
    (!id.is_empty()).then(|| id.to_string())
}

/// Text of a theme file, or `None` when it is missing or cannot be read.
fn read_style_file(path: &str) -> Option<String> {
    let file = FileAccess::open(path, ModeFlags::READ)?;
    Some(file.get_as_text().to_string())
}

/// Saves `id` as the last map opened. A failure is a warning: the game still runs.
fn write_last_map(id: &str) {
    let Some(mut file) = FileAccess::open(LAST_MAP_PATH, ModeFlags::WRITE) else {
        godot_warn!("cannot save the last map to {LAST_MAP_PATH}");
        return;
    };
    if !file.store_string(id) {
        godot_warn!("cannot save the last map to {LAST_MAP_PATH}");
    }
}

#[godot_api]
impl INode3D for EarthWorld {
    fn ready(&mut self) {
        self.base_mut().set_process(true);
        // Only the screen and the state are set up here: the map is opened in the first `process`,
        // after the screen has been drawn, so the expensive work never runs over a grey frame.
        self.lang = Some(language(&Os::singleton().get_locale().to_string()));
        self.loading_started = Some(Instant::now());
        self.show_loading();
        self.stage = LoadingStage::OpeningMap;
        self.stage_started = Some(Instant::now());
        self.first_frame_pending = true;
        self.loading_hold_s = self.loading_hold_seconds();
    }

    fn process(&mut self, delta: f64) {
        if self.stage != LoadingStage::Ready {
            self.loading_tick();
            return;
        }
        // `--loading-hold` keeps the last stage up for a while (for a capture), still with the
        // normal streaming running; without it the screen was already closed by `enter_ready`.
        if let Some(until) = self.loading_hold_until {
            if Instant::now() >= until {
                self.loading_hold_until = None;
                self.finish_loading();
            }
        }

        let frame_start = Instant::now();
        let (Some(index), Some(p)) = (self.index.clone(), self.car_position()) else {
            return;
        };
        let tile_m = index.meta.tile_size_m;
        let radius = self.view_radius_m as f64;

        // Free the tiles that left the view, with the hysteresis margin.
        let stale: Vec<(i32, i32)> = self
            .loaded
            .keys()
            .copied()
            .filter(|&t| is_stale(t, tile_m, p, radius, HYSTERESIS_M))
            .collect();
        for t in stale {
            self.lamp_posts.remove(&t);
            self.built_cell.remove(&t);
            if let Some(mut node) = self.loaded.remove(&t) {
                node.queue_free();
            }
        }

        self.collect_results();
        self.collect_far();

        // Distant terrain: rebuilt in the background when the loaded tiles or the car's window
        // change; one node per finished build, the previous one freed.
        let loaded: HashSet<(i32, i32)> = self.loaded.keys().copied().collect();
        if let Some(node) = self.lod.as_mut().and_then(|lod| lod.update(p, &loaded)) {
            self.base_mut().add_child(&node);
            if let Some(mut old) = self.lod_node.replace(node) {
                old.queue_free();
            }
        }

        // Instantiate the geometry that came back, within the per-frame budget.
        let build_start = Instant::now();
        let mut built = 0;
        while built < TILES_PER_FRAME {
            let Some((t, geometry)) = self.done.pop_front() else {
                break;
            };
            if self.built_cell.get(&t) == Some(&geometry.mesh_cell_m)
                || is_stale(t, tile_m, p, radius, HYSTERESIS_M)
            {
                continue;
            }
            // A tile rebuilt at another side replaces its old node only now, so it never has a gap.
            if let Some(mut old) = self.loaded.remove(&t) {
                old.queue_free();
            }
            self.add_tile(t, &geometry, tile_m);
            built += 1;
        }
        if built > 0 {
            self.worst_build = self.worst_build.max(build_start.elapsed().as_secs_f64());
        }

        // Ask the loader for the tiles in view that are neither loaded nor on their way.
        for t in tiles_in_range(index.coords(), tile_m, p, radius) {
            if self.requested.len() >= MAX_IN_FLIGHT {
                break;
            }
            let cell = mesh_cell_for(
                distance_to_tile(t, tile_m, p),
                self.built_cell.get(&t).copied(),
            );
            if self.built_cell.get(&t) == Some(&cell)
                || self.requested.contains(&t)
                || self.failed.contains(&t)
                || self
                    .done
                    .iter()
                    .any(|(d, g)| *d == t && g.mesh_cell_m == cell)
            {
                continue;
            }
            let Some(tx) = &self.requests else {
                break;
            };
            if tx.send((t, cell)).is_ok() {
                self.requested.insert(t);
            }
        }

        self.lamp_timer += delta;
        if self.lamp_timer >= LAMP_UPDATE_S {
            self.lamp_timer = 0.0;
            self.update_lamp_lights(p);
        }

        self.worst_frame = self.worst_frame.max(frame_start.elapsed().as_secs_f64());
        self.stats_timer += delta;
        if self.stats_timer >= STATS_PERIOD_S {
            godot_print!(
                "stream: {} tiles loaded, {} requested, {} waiting; worst frame {:.2} ms, worst tile build {:.2} ms; lamps {} lit of {}",
                self.loaded.len(),
                self.requested.len(),
                self.done.len(),
                self.worst_frame * 1000.0,
                self.worst_build * 1000.0,
                self.lamps_lit,
                self.lamps_total
            );
            if let Some(far) = &self.far {
                godot_print!(
                    "far lights lit: {} of {}; instances {}, triangles {}; worst frame {:.2} ms",
                    far.lit,
                    far.count(),
                    far.count(),
                    far.count() * 2,
                    self.worst_frame * 1000.0
                );
            }
            self.stats_timer = 0.0;
            self.worst_frame = 0.0;
            self.worst_build = 0.0;
        }
    }
}

impl EarthWorld {
    /// Adds the loading screen of item 123, above the 3D view. It ignores the mouse. A missing
    /// scene is a warning: the stages still run, only without the card.
    fn show_loading(&mut self) {
        let scene: Gd<PackedScene> = match try_load("res://loading.tscn") {
            Ok(scene) => scene,
            Err(e) => {
                godot_warn!("loading screen res://loading.tscn not loaded ({e:?}): opening without it");
                return;
            }
        };
        let Some(node) = scene.instantiate() else {
            godot_warn!("loading screen res://loading.tscn could not be instantiated");
            return;
        };
        if let Ok(mut layer) = node.clone().try_cast::<CanvasLayer>() {
            layer.set_layer(LOADING_CANVAS_LAYER);
        }
        self.base_mut().add_child(&node);
        self.loading = Some(node);
        self.set_loading_text("set_title", "intro.title");
        self.set_loading_text("set_hint", "loading.hint");
        self.set_loading(
            "loading.opening_map",
            stage_progress(LoadingStage::OpeningMap, 0, 1),
        );
    }

    /// Calls `method` on the screen with one translated line (title or hint).
    fn set_loading_text(&mut self, method: &str, key: &str) {
        let line = ui_text(key, self.lang.unwrap_or(Lang::En));
        if let Some(mut loading) = self.loading.clone() {
            loading.call(method, &[line.to_variant()]);
        }
    }

    /// Sends one line of text (in the player's language) and a 0..1 progress to the screen.
    fn set_loading(&mut self, key: &str, progress: f64) {
        let line = ui_text(key, self.lang.unwrap_or(Lang::En));
        if let Some(mut loading) = self.loading.clone() {
            let args = [line.to_variant(), progress.clamp(0.0, 1.0).to_variant()];
            loading.call("set_stage", &args);
        }
    }

    /// Hides and frees the screen. Idempotent, so the failure path and `ready` can both call it.
    fn finish_loading(&mut self) {
        let Some(mut loading) = self.loading.take() else {
            return;
        };
        loading.call("finish", &[]);
        loading.queue_free();
        if let Some(started) = self.loading_started {
            godot_print!(
                "loading: screen closed at {:.1} ms",
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
    }

    /// No map, or a map that could not be read: close the screen and open the world map with
    /// `home.no_map`, exactly as the old `ready` did on the no-map path. Never leaves it stuck.
    fn fail_loading(&mut self) {
        godot_print!(
            "loading: stage '{}' failed, closing the screen and opening the world map",
            stage_label(self.stage)
        );
        self.finish_loading();
        self.stage = LoadingStage::Ready;
        self.pending_index = None;
        self.pending_cache = None;
        self.pending_picker_key = None;
        self.pending_car_position = None;
        self.pending_tiles.clear();
        self.pending_built = 0;
        self.pending_total = 0;
        self.signals().no_map().emit();
        self.add_map_view(Some("home.no_map"));
    }

    /// One loading step per frame. The very first frame only draws the screen; the cost of the
    /// map starts on the next one.
    fn loading_tick(&mut self) {
        if self.first_frame_pending {
            self.first_frame_pending = false;
            return;
        }
        match self.stage {
            LoadingStage::OpeningMap => self.stage_opening_map(),
            LoadingStage::ReadingMap => self.stage_reading_map(),
            LoadingStage::BuildingTiles => self.stage_building_tiles(),
            LoadingStage::PlacingCar => self.stage_placing_car(),
            LoadingStage::Environment => self.stage_environment(),
            LoadingStage::StartingStream => self.stage_starting_stream(),
            LoadingStage::WorldMap => self.stage_world_map(),
            LoadingStage::Ready => {}
        }
    }

    /// Moves to `stage`, logs how long the previous one took and updates the screen.
    fn begin_stage(&mut self, stage: LoadingStage) {
        if let Some(started) = self.stage_started {
            godot_print!(
                "loading: stage '{}' took {:.1} ms",
                stage_label(self.stage),
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
        self.stage = stage;
        self.stage_started = Some(Instant::now());
        let progress = stage_progress(stage, self.pending_built, self.pending_total);
        self.set_loading(stage_key(stage), progress);
    }

    /// The map is open and the car is in the tree: it can drive from now on. The world map stage
    /// already showed its own line while its heavy load ran; the screen now closes itself (or
    /// after `--loading-hold`, for a capture).
    fn enter_ready(&mut self) {
        self.begin_stage(next_stage(LoadingStage::WorldMap));
        if let Some(started) = self.loading_started {
            godot_print!(
                "loading: first playable frame at {:.1} ms",
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
        self.pending_index = None;
        self.pending_cache = None;
        self.pending_tiles.clear();
        // The screen closes on this first playable frame, unless a capture asked to hold the
        // last stage (`--loading-hold`).
        if self.loading_hold_s > 0.0 {
            self.loading_hold_until =
                Some(Instant::now() + Duration::from_secs_f64(self.loading_hold_s));
        } else {
            self.finish_loading();
        }
    }

    /// Stage 1: read the theme and open the map (metadata only; no tile is read here).
    fn stage_opening_map(&mut self) {
        self.load_style();
        let (cache_root_res, dev_cache) = self.resolved_cache_root();
        // The property shows the root in use, so the bot and the capture read the same cache.
        self.cache_root = cache_root_res.clone();
        let cache_root = PathBuf::from(
            ProjectSettings::singleton()
                .globalize_path(&cache_root_res)
                .to_string(),
        );
        let id = self.resolved_map_id(dev_cache);
        let opened = if id.is_empty() {
            godot_print!("no map: no map_id, no last map and no development cache");
            None
        } else {
            match open_map(&cache_root, &id) {
                Ok(index) => Some(index),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    godot_print!("no map: '{id}' not found in {}", cache_root.display());
                    None
                }
                Err(e) => {
                    godot_error!("cannot open map '{id}' from {}: {e}", cache_root.display());
                    None
                }
            }
        };
        let Some(index) = opened else {
            self.fail_loading();
            return;
        };
        write_last_map(&id);
        godot_print!(
            "map '{id}' opened: {} tiles, streamed within {} m",
            index.coords().len(),
            self.view_radius_m
        );
        self.active_cache = Some(cache_root.clone());
        self.active_id = id.clone();
        self.pending_cache = Some(cache_root);
        self.pending_map_id = id;
        self.pending_index = Some(Arc::new(index));
        self.begin_stage(next_stage(LoadingStage::OpeningMap));
    }

    /// Stage 2: home, spawn search, materials and the list of spawn tiles (built one per frame).
    fn stage_reading_map(&mut self) {
        let (Some(index), Some(cache_root)) = (self.pending_index.clone(), self.pending_cache.clone())
        else {
            godot_error!("loading: map state is missing, cannot read the map");
            self.fail_loading();
            return;
        };
        let id = self.pending_map_id.clone();

        let home: Option<Home> = home_store::load();
        if let Some(h) = home {
            crate::environment::sky::set_home(h.lat, h.lon);
        }
        self.start_gps(&cache_root.display().to_string(), &id);
        self.terrain_mat = Some(terrain_material());
        self.road_mat = Some(road_material());
        self.vertex_mat = Some(vertex_material());
        self.building_mat = building_material();
        if self.building_mat.is_none() {
            godot_warn!("building shader not loaded: buildings have no windows");
        }
        self.lamp_mats = LampMaterials::new();
        if self.lamp_mats.is_none() {
            godot_warn!("lamp shaders not loaded: street lamps are not drawn");
        }
        self.lamp_lights = Some(LampLights::new());

        // Spawn on the road nearest to the saved home when it is inside the loaded map. Without a
        // saved home, or with one outside the map, spawn on the road nearest to the map center
        // and open the world map so the player chooses (no grey screen).
        let grid = MapGrid::from_meta(&index.meta);
        let saved_local = home.and_then(|h| {
            inside_loaded_map(&grid, index.meta.radius_m, h.lat, h.lon, |t| {
                index.has_tile(t.0, t.1)
            })
        });
        let saved_pose = match saved_local {
            Some(local) => match find_spawn_near(&index, local, self.view_radius_m as f64) {
                Ok(pose) => pose,
                Err(e) => {
                    godot_error!("cannot read map '{id}': {e}");
                    self.fail_loading();
                    return;
                }
            },
            None => None,
        };
        let picker_key = match (home, saved_pose) {
            (Some(_), Some(_)) => None,
            (Some(_), None) if saved_local.is_some() => Some("home.no_road_start".to_string()),
            (Some(_), None) => Some("home.outside_saved".to_string()),
            (None, _) => Some("home.none_start".to_string()),
        };
        let pose = match saved_pose {
            Some(pose) => Ok(Some(pose)),
            None => find_spawn(&index, self.view_radius_m as f64),
        };
        let (x, y, yaw) = match pose {
            Ok(Some(pose)) => (pose.x, pose.y, pose.yaw),
            Ok(None) => {
                godot_warn!("map '{id}' has no road to spawn on, spawning at the map center");
                (0.0, 0.0, std::f32::consts::PI)
            }
            Err(e) => {
                godot_error!("cannot read map '{id}': {e}");
                self.fail_loading();
                return;
            }
        };
        let tile_m = index.meta.tile_size_m;
        let spawn_tile = ((x / tile_m).floor() as i32, (y / tile_m).floor() as i32);
        let near = match index.neighborhood(spawn_tile.0, spawn_tile.1) {
            Ok(map) => map,
            Err(e) => {
                godot_error!("cannot read tiles at the spawn point of '{id}': {e}");
                self.fail_loading();
                return;
            }
        };
        let Some(height) = near.height_at(x, y) else {
            godot_error!("map '{id}' has no tile at the spawn point, car not spawned");
            self.fail_loading();
            return;
        };

        self.pending_car_position = Some(Vector3::new(
            x as f32,
            height + SPAWN_LIFT_M,
            -(y as f32),
        ));
        self.pending_car_yaw = yaw;
        self.pending_picker_key = picker_key;
        // The car lands on the spawn tile and its neighbors; these are built before it exists.
        self.pending_tiles.clear();
        for ty in spawn_tile.1 - 1..=spawn_tile.1 + 1 {
            for tx in spawn_tile.0 - 1..=spawn_tile.0 + 1 {
                if index.has_tile(tx, ty) {
                    self.pending_tiles.push_back((tx, ty));
                }
            }
        }
        self.pending_built = 0;
        self.pending_total = self.pending_tiles.len().max(1);
        self.begin_stage(next_stage(LoadingStage::ReadingMap));
    }

    /// Stage 3: one spawn tile per frame, so the card repaints between tiles.
    fn stage_building_tiles(&mut self) {
        let Some(index) = self.pending_index.clone() else {
            godot_error!("loading: map state is missing, cannot build the terrain");
            self.fail_loading();
            return;
        };
        let Some(t) = self.pending_tiles.pop_front() else {
            self.begin_stage(next_stage(LoadingStage::BuildingTiles));
            return;
        };
        self.load_now(&index, t);
        self.pending_built += 1;
        let progress =
            stage_progress(LoadingStage::BuildingTiles, self.pending_built, self.pending_total);
        self.set_loading("loading.building_tiles", progress);
        if self.pending_tiles.is_empty() {
            self.begin_stage(next_stage(LoadingStage::BuildingTiles));
        }
    }

    /// Stage 4: the car, on the spawn tiles that are already built.
    fn stage_placing_car(&mut self) {
        let Some(position) = self.pending_car_position else {
            godot_error!("loading: no spawn position, car not spawned");
            self.fail_loading();
            return;
        };
        // The car is instantiated here, on the built spawn tiles, but only enters the tree in
        // `StartingStream`: the capture and the drive bot look for its Camera, and they must not
        // find it before the loader and the map view are running.
        match spawn_car(position, self.pending_car_yaw) {
            Some(car) => self.car = Some(car),
            None => {
                godot_error!("cannot instantiate res://car.tscn");
                self.fail_loading();
                return;
            }
        }
        self.begin_stage(next_stage(LoadingStage::PlacingCar));
    }

    /// Stage 5: the `Environment` clock and the pool of real lamp lights.
    fn stage_environment(&mut self) {
        self.add_environment();
        self.add_lamp_lights();
        self.begin_stage(next_stage(LoadingStage::Environment));
    }

    /// Stage 6: the loader thread, distant lights, terrain LOD and the car in the tree.
    fn stage_starting_stream(&mut self) {
        let (Some(index), Some(cache_root)) = (self.pending_index.clone(), self.pending_cache.clone())
        else {
            godot_error!("loading: map state is missing, cannot start the stream");
            self.fail_loading();
            return;
        };
        let id = self.pending_map_id.clone();
        if self.far_lights_enabled {
            let path = cache_root.join(&id).join("far_lights.bin");
            self.far_loader = Some(spawn_load(path, self.far_lights_max.max(0) as usize));
        }
        let car = self.car.clone();
        let material = self.terrain_mat.clone();
        let road_mat = self.road_mat.clone();
        if let (Some(car), Some(material), Some(road_mat)) = (car, material, road_mat) {
            lod::configure_view(self.base().get_parent(), &car);
            self.lod = Some(TerrainLod::start(
                &cache_root,
                &id,
                &index.meta,
                material,
                road_mat,
            ));
        }
        self.start_loader(index);
        // The car (and its Camera) enters the tree only now, when streaming is already running:
        // the tools that wait for the Camera then see a fully playable world.
        if let Some(car) = self.car.clone() {
            self.base_mut().add_child(&car);
        }
        self.begin_stage(next_stage(LoadingStage::StartingStream));
    }

    /// Stage 7: the world map view, whose world data load is the heaviest step left. Its line is
    /// set by `begin_stage` before this call, so the card can draw it while the load blocks.
    fn stage_world_map(&mut self) {
        let key = self.pending_picker_key.clone();
        self.add_map_view(key.as_deref());
        self.enter_ready();
    }

    /// `--loading-hold <seconds>` (or `--loading-hold=<seconds>`): keep the last stage visible
    /// for a capture. 0 when absent.
    fn loading_hold_seconds(&self) -> f64 {
        let args = Os::singleton().get_cmdline_user_args();
        let mut iter = args.as_slice().iter();
        while let Some(arg) = iter.next() {
            let arg = arg.to_string();
            if arg == "--loading-hold" {
                return iter
                    .next()
                    .and_then(|v| v.to_string().trim().parse::<f64>().ok())
                    .unwrap_or(0.0)
                    .max(0.0);
            }
            if let Some(value) = arg.strip_prefix("--loading-hold=") {
                return value.trim().parse::<f64>().unwrap_or(0.0).max(0.0);
            }
        }
        0.0
    }

    /// Reads the active theme and installs it: `--style <id>` from the Godot user args wins, then
    /// the `style_id` export, then the default theme. A missing or broken file logs one warning
    /// and keeps the built-in defaults (equal to `lowpoly.json`).
    fn load_style(&self) {
        let args: Vec<String> = Os::singleton()
            .get_cmdline_user_args()
            .as_slice()
            .iter()
            .map(|arg| arg.to_string())
            .collect();
        let exported = self.style_id.to_string();
        let id = style::theme_id_from_args(&args)
            .or_else(|| {
                let id = exported.trim();
                (!id.is_empty()).then(|| id.to_string())
            })
            .unwrap_or_else(|| style::DEFAULT_ID.to_string());
        let path = format!("res://styles/{id}.json");
        let (theme, source) = match read_style_file(&path) {
            Some(text) => match style::parse(&text, &id) {
                Ok(theme) => (theme, "file"),
                Err(e) => {
                    godot_warn!("style '{path}' is broken ({e}): using the built-in defaults");
                    (style::builtin(&id), "built-in defaults")
                }
            },
            None => {
                godot_warn!("style '{path}' not found: using the built-in defaults");
                (style::builtin(&id), "built-in defaults")
            }
        };
        let palette = theme.world.palette;
        let (light, ui) = (theme.light, theme.ui);
        godot_print!(
            "style '{}' active from {}: flat shading {}, bevel {} m in {} segment(s), shadow softness {}; palette grass {}, road {}, buildings {} {} {} {}, lamp {}, car {}; light sun {} / {}, sky {} / {}, shadow {}; ui panel {}, ink {}, accent {}, border {} {} px, radius {} px",
            theme.id,
            source,
            theme.world.flat_shading,
            theme.world.bevel_m,
            theme.world.bevel_segments,
            theme.world.shadow_softness,
            style::to_hex(palette.grass),
            style::to_hex(palette.road),
            style::to_hex(palette.buildings[0]),
            style::to_hex(palette.buildings[1]),
            style::to_hex(palette.buildings[2]),
            style::to_hex(palette.buildings[3]),
            style::to_hex(palette.lamp),
            style::to_hex(palette.car),
            style::to_hex(light.sun_high),
            style::to_hex(light.sun_low),
            style::to_hex(light.sky_day),
            style::to_hex(light.sky_horizon),
            style::to_hex(light.shadow_tint),
            style::to_hex(ui.panel),
            style::to_hex(ui.ink),
            style::to_hex(ui.accent),
            style::to_hex(ui.border),
            ui.border_width,
            ui.corner_radius
        );
        style::install(theme);
    }

    /// The cache root to open, and whether it is the repo cache of development. The repo cache
    /// replaces the default root only when the default is kept and the folder exists.
    fn resolved_cache_root(&self) -> (GString, bool) {
        let dev = GString::from(DEV_CACHE_ROOT);
        let dev_exists = PathBuf::from(
            ProjectSettings::singleton()
                .globalize_path(&dev)
                .to_string(),
        )
        .is_dir();
        if dev_exists && self.cache_root.to_string() == DEFAULT_CACHE_ROOT {
            (dev, true)
        } else {
            (self.cache_root.clone(), false)
        }
    }

    /// Loads the map in use into the world's `Gps` node (item 112). The node reads the map in a
    /// thread of its own, so the frame does not wait for it. Without the node there is no guide.
    /// Any destination of the previous map is dropped (its local meters belong to another map).
    fn start_gps(&self, cache_root: &str, id: &str) {
        let Some(mut gps) = self.base().get_node_or_null("Gps") else {
            godot_warn!("no Gps node under the world: no route guide");
            return;
        };
        gps.call("clear", &[]);
        gps.call(
            "load_map",
            &[
                GString::from(cache_root).to_variant(),
                GString::from(id).to_variant(),
            ],
        );
    }

    /// The map to open: the `map_id` given, else the last map opened, else the development map
    /// (only with the repo cache). Empty when there is none.
    fn resolved_map_id(&self, dev_cache: bool) -> String {
        let given = self.map_id.to_string();
        if !given.is_empty() {
            return given;
        }
        if let Some(last) = read_last_map() {
            return last;
        }
        if dev_cache {
            DEV_DEFAULT_MAP.into()
        } else {
            String::new()
        }
    }

    /// The other complete map of `cache` that holds (`lat`, `lon`) and has a road near that point:
    /// its id, index and spawn pose. Folders that are not maps are skipped without a message.
    ///
    /// When two baked maps overlap, the smallest one wins (then the id, for determinism): a small
    /// map around the chosen place is more specific than a large map that happens to reach it. It
    /// also keeps the road search cheap: the big map is only opened when the small one has no road.
    fn find_other_map(
        &self,
        cache: &Path,
        lat: f64,
        lon: f64,
    ) -> Option<(String, MapIndex, SpawnPose)> {
        let entries = std::fs::read_dir(cache).ok()?;
        let mut ids: Vec<String> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|id| *id != self.active_id)
            .collect();
        ids.sort();
        // Reading the metadata and listing the tiles is cheap: find the maps that hold the point
        // before reading any road.
        let mut holding: Vec<(f64, String, MapIndex, [f64; 2])> = Vec::new();
        for id in ids {
            let Ok(index) = open_map(cache, &id) else {
                continue;
            };
            let grid = MapGrid::from_meta(&index.meta);
            let Some(local) = inside_loaded_map(&grid, index.meta.radius_m, lat, lon, |t| {
                index.has_tile(t.0, t.1)
            }) else {
                continue;
            };
            holding.push((index.meta.radius_m, id, index, local));
        }
        holding.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        for (_, id, index, local) in holding {
            match find_spawn_near(&index, local, self.view_radius_m as f64) {
                Ok(Some(pose)) => return Some((id, index, pose)),
                Ok(None) => continue,
                Err(e) => {
                    godot_error!("cannot read map '{id}' for the car move: {e}");
                    continue;
                }
            }
        }
        None
    }

    /// Replaces the loaded map by `index` (folder `id` under `cache`) and puts the car at `pose`
    /// (item 106). The streams of the old map are dropped and the new ones start as at the opening:
    /// loader, distant lights, terrain LOD and GPS. The spawn tiles are built here, as in a car move.
    /// False when the spawn point has no terrain (nothing changes then).
    fn switch_map(
        &mut self,
        cache: PathBuf,
        id: String,
        index: MapIndex,
        pose: SpawnPose,
    ) -> bool {
        let tile_m = index.meta.tile_size_m;
        let spawn_tile = (
            (pose.x / tile_m).floor() as i32,
            (pose.y / tile_m).floor() as i32,
        );
        let height = match index.neighborhood(spawn_tile.0, spawn_tile.1) {
            Ok(near) => near.height_at(pose.x, pose.y),
            Err(e) => {
                godot_error!("cannot read tiles at the new road of '{id}': {e}");
                return false;
            }
        };
        let Some(height) = height else {
            godot_print!("map '{id}' has no terrain at the new road, the car stays");
            return false;
        };
        let index = Arc::new(index);
        let grid = MapGrid::from_meta(&index.meta);
        let previous = self.active_id.clone();
        let started = Instant::now();
        self.clear_streams();
        write_last_map(&id);
        self.active_cache = Some(cache.clone());
        self.active_id = id.clone();
        self.map_grid.set(Some(grid));
        self.start_gps(&cache.display().to_string(), &id);
        if self.far_lights_enabled {
            let path = cache.join(&id).join("far_lights.bin");
            self.far_loader = Some(spawn_load(path, self.far_lights_max.max(0) as usize));
        }
        if let (Some(material), Some(road_mat)) =
            (self.terrain_mat.clone(), self.road_mat.clone())
        {
            self.lod = Some(TerrainLod::start(
                &cache,
                &id,
                &index.meta,
                material,
                road_mat,
            ));
        }
        self.start_loader(index.clone());
        if let Some(mut car) = self.car.clone() {
            car.set_position(Vector3::new(
                pose.x as f32,
                height + SPAWN_LIFT_M,
                -(pose.y as f32),
            ));
            car.set_rotation(Vector3::new(0.0, pose.yaw, 0.0));
            if let Ok(mut body) = car.try_cast::<RigidBody3D>() {
                body.set_linear_velocity(Vector3::ZERO);
                body.set_angular_velocity(Vector3::ZERO);
            }
        }
        for ty in spawn_tile.1 - 1..=spawn_tile.1 + 1 {
            for tx in spawn_tile.0 - 1..=spawn_tile.0 + 1 {
                self.load_now(&index, (tx, ty));
            }
        }
        godot_print!(
            "map switched from '{previous}' to '{id}' ({} tiles) in {:.0} ms",
            index.coords().len(),
            started.elapsed().as_secs_f64() * 1000.0
        );
        true
    }

    /// Drops everything streamed from the map in use: its tile nodes, terrain LOD, distant lights,
    /// pending geometry and loader (the loader thread ends with its request sender). The car, the
    /// environment and the real lamp lights stay. The nodes are freed now, not queued: the switch
    /// happens with the game paused, and the new map may reuse the same tile names and coordinates.
    fn clear_streams(&mut self) {
        for (_, node) in self.loaded.drain() {
            node.free();
        }
        if let Some(node) = self.lod_node.take() {
            node.free();
        }
        if let Some(far) = self.far.take() {
            far.node().free();
        }
        self.lod = None;
        self.far_loader = None;
        self.requests = None;
        self.results = None;
        self.index = None;
        self.lamp_posts.clear();
        self.built_cell.clear();
        self.requested.clear();
        self.failed.clear();
        self.done.clear();
        self.lamps_lit = 0;
        self.lamps_total = 0;
    }

    /// Starts the loader thread. It builds tile geometry for each request, in order, and
    /// stops when this node (and with it the request sender) is freed.
    fn start_loader(&mut self, index: Arc<MapIndex>) {
        let (req_tx, req_rx) = mpsc::channel::<((i32, i32), f64)>();
        let (res_tx, res_rx) = mpsc::channel();
        self.map_grid.set(Some(MapGrid::from_meta(&index.meta)));
        let worker_index = index.clone();
        thread::spawn(move || {
            for ((x, y), cell) in req_rx {
                let result = worker_index.tile_geometry_at(x, y, cell);
                if res_tx.send(((x, y), result)).is_err() {
                    break;
                }
            }
        });
        self.index = Some(index);
        self.requests = Some(req_tx);
        self.results = Some(res_rx);
    }

    fn collect_results(&mut self) {
        let Some(rx) = &self.results else {
            return;
        };
        loop {
            match rx.try_recv() {
                Ok((t, Ok(geometry))) => {
                    self.requested.remove(&t);
                    self.done.push_back((t, geometry));
                }
                Ok((t, Err(e))) => {
                    self.requested.remove(&t);
                    self.failed.insert(t);
                    godot_error!("cannot load tile {} {}: {e}", t.0, t.1);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    /// Builds tile `t` now, on the main thread (spawn area only).
    fn load_now(&mut self, index: &MapIndex, t: (i32, i32)) {
        if !index.has_tile(t.0, t.1) || self.loaded.contains_key(&t) {
            return;
        }
        let tile_m = index.meta.tile_size_m;
        let p = self.car_position().unwrap_or([0.0, 0.0]);
        let cell = mesh_cell_for(distance_to_tile(t, tile_m, p), None);
        match index.tile_geometry_at(t.0, t.1, cell) {
            Ok(geometry) => self.add_tile(t, &geometry, tile_m),
            Err(e) => {
                godot_error!("cannot load tile {} {}: {e}", t.0, t.1);
            }
        }
    }

    /// Builds the distant lights when the background load is back (the MultiMesh buffer is set in
    /// one call). A missing file means no distant lights, without a message.
    fn collect_far(&mut self) {
        let Some(rx) = &self.far_loader else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(io::Error::other("loader gone")),
        };
        self.far_loader = None;
        match result {
            Ok(load) => {
                let (file, kept, secs) = (load.file_points, load.loaded_points, load.secs);
                let rss = rss_mb();
                if let Some(far) = DistantLights::new(load) {
                    self.base_mut().add_child(&far.node());
                    godot_print!(
                        "far lights: loaded {kept} of {file} in {:.0} ms in the background, resident {:.0} MB",
                        secs * 1000.0,
                        rss.unwrap_or(0.0)
                    );
                    self.far = Some(far);
                } else {
                    godot_warn!("far lights shader not loaded: distant lights are not drawn");
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                godot_warn!("cannot read far_lights.bin: {e}");
            }
        }
    }

    /// Creates the nodes of one tile: terrain body, and road and building bodies when there are some.
    fn add_tile(&mut self, t: (i32, i32), geometry: &TileGeometry, tile_m: f64) {
        let (Some(terrain_mat), Some(road_mat), Some(vertex_mat)) = (
            self.terrain_mat.clone(),
            self.road_mat.clone(),
            self.vertex_mat.clone(),
        ) else {
            return;
        };
        let mut node = Node3D::new_alloc();
        // Named by tile and body, so the drive bot's stop reports say what the car touched.
        node.set_name(&format!("Tile_{}_{}", t.0, t.1));
        // Terrain of tiles with ground cover carries its colors; older tiles keep the albedo.
        let terrain = if geometry.terrain.colors.is_empty() {
            &terrain_mat
        } else {
            &vertex_mat
        };
        let origin = [t.0 as f64 * tile_m, t.1 as f64 * tile_m];
        let mut terrain_body = terrain_body(
            &geometry.terrain,
            terrain,
            &geometry.collision,
            origin,
            tile_m,
        );
        terrain_body.set_name("Terrain");
        node.add_child(&terrain_body);
        if !geometry.roads.is_empty() {
            // Roads are solid too, so the wheels rest on them instead of on the terrain below.
            let mut road_body = solid_body(&geometry.roads, &road_mat);
            road_body.set_name("Roads");
            node.add_child(&road_body);
        }
        if !geometry.buildings.is_empty() {
            // One mesh and one collision body per tile, for the buildings of that tile. The
            // collision is each building's convex prism, not a trimesh of its walls.
            let prisms = || geometry.building_prisms.iter().map(|p| p.points.as_slice());
            let mut body = match &self.building_mat {
                Some(mat) => building_body(&geometry.buildings, mat, prisms()),
                None => building_body(&geometry.buildings, &vertex_mat, prisms()),
            };
            body.set_name("Buildings");
            node.add_child(&body);
        }
        if let Some(mats) = &self.lamp_mats {
            if !geometry.lamps.is_empty() {
                node.add_child(&lamp_node(&geometry.lamps, mats, self.lamp_glow_enabled));
            }
        }
        self.lamp_posts.insert(t, geometry.lamps.clone());
        self.base_mut().add_child(&node);
        self.loaded.insert(t, node);
        self.built_cell.insert(t, geometry.mesh_cell_m);
    }

    /// Adds the world map (M) on its own canvas layer. It starts hidden; the car and the map's
    /// frame give it the car's place in lat/lon, when there is a loaded map. `open_with` opens it
    /// at once with that message (the player has no home yet, or it is outside the map).
    fn add_map_view(&mut self, open_with: Option<&str>) {
        let has_index = self.index.is_some();
        let car = self.car.clone();
        let origin = self.to_gd().upcast::<Node3D>();
        let mut layer = CanvasLayer::new_alloc();
        layer.set_layer(MAP_CANVAS_LAYER);
        let mut map = WorldMapView::new_alloc();
        if let (Some(car), true) = (car, has_index) {
            map.bind_mut().start(car, origin, self.map_grid.clone());
        }
        layer.add_child(&map);
        self.base_mut().add_child(&layer);
        if let Some(key) = open_with {
            map.bind_mut().open_automatically(key);
        }
    }

    /// Adds the `Environment` clock node. It drives the scene's WorldEnvironment and replaces
    /// the scene's Sun with its own lights; runs before `configure_view` sets the fog.
    fn add_environment(&mut self) {
        let world_env = self
            .base()
            .get_parent()
            .and_then(|p| p.get_node_or_null("WorldEnvironment"))
            .and_then(|n| n.try_cast::<godot::classes::WorldEnvironment>().ok());
        let old_sun = self.base().get_node_or_null("Sun");
        let mut clock = EnvironmentClock::new_alloc();
        clock.bind_mut().attach(world_env, old_sun);
        clock.set_name("Environment");
        self.clock = Some(clock.clone());
        self.base_mut().add_child(&clock);
    }

    /// Adds the pool of real lights under the world node.
    fn add_lamp_lights(&mut self) {
        let nodes = self
            .lamp_lights
            .as_ref()
            .map(|l| l.nodes())
            .unwrap_or_default();
        for light in nodes {
            self.base_mut().add_child(&light);
        }
    }

    /// Moves the real lights to the lit lamps nearest the car (within `LAMP_REAL_REACH_M`).
    fn update_lamp_lights(&mut self, p: [f64; 2]) {
        let Some(clock) = &self.clock else {
            return;
        };
        let (now, on_since) = clock.bind().lamp_clock();
        let total: usize = self.lamp_posts.values().map(Vec::len).sum();
        let mut lit: Vec<(f64, Vector3)> = Vec::new();
        let mut lit_total = 0;
        if on_since.is_some() {
            for post in self.lamp_posts.values().flatten() {
                if !is_lit(now, on_since, delay_s(post.id)) {
                    continue;
                }
                lit_total += 1;
                let d = (post.base[0] as f64 - p[0]).hypot(-(post.base[2] as f64) - p[1]);
                if d <= LAMP_REAL_REACH_M {
                    lit.push((d, head_position(post)));
                }
            }
        }
        self.lamps_lit = lit_total;
        self.lamps_total = total;
        if let Some(far) = self.far.as_mut() {
            far.update(now, on_since);
        }
        lit.sort_by(|a, b| a.0.total_cmp(&b.0));
        let heads: Vec<Vector3> = lit.into_iter().take(LAMP_POOL).map(|(_, v)| v).collect();
        let active = self.lamp_real_lights.clamp(0, LAMP_POOL as i64) as usize;
        if let Some(lights) = self.lamp_lights.as_mut() {
            lights.place(&heads, active);
        }
    }

    fn car_position(&self) -> Option<[f64; 2]> {
        let car = self.car.as_ref()?;
        let local = self.base().to_local(car.get_global_position());
        Some([local.x as f64, -local.z as f64])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_stages_advance_in_order_and_progress_never_goes_back() {
        let order = [
            LoadingStage::OpeningMap,
            LoadingStage::ReadingMap,
            LoadingStage::BuildingTiles,
            LoadingStage::PlacingCar,
            LoadingStage::Environment,
            LoadingStage::StartingStream,
            LoadingStage::WorldMap,
            LoadingStage::Ready,
        ];
        for pair in order.windows(2) {
            assert_eq!(next_stage(pair[0]), pair[1]);
        }
        assert_eq!(next_stage(LoadingStage::Ready), LoadingStage::Ready);

        let mut previous = 0.0;
        for stage in order {
            for built in 0..=9 {
                let progress = stage_progress(stage, built, 9);
                assert!(progress >= previous, "{stage:?} went back at {built}");
                assert!((0.0..=1.0).contains(&progress));
                previous = progress;
            }
            assert!(!stage_key(stage).is_empty());
            assert!(!stage_label(stage).is_empty());
        }
        assert_eq!(stage_progress(LoadingStage::Ready, 0, 9), 1.0);
    }
}
