//! Godot node that streams a baked map around the car. The map is opened without reading
//! its tiles; the car is spawned on the road nearest to home (the map center), facing south,
//! on terrain loaded synchronously. Then the tiles within `view_radius_m` of the car are
//! built in the background and instantiated on the main thread, a few per frame, and tiles
//! that fall beyond the radius plus a margin are freed.
use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use godot::classes::{
    CanvasLayer, INode3D, MeshInstance3D, Node3D, ProjectSettings, RigidBody3D, ShaderMaterial,
    StandardMaterial3D,
};
use godot::prelude::*;

use crate::environment::lamps::{delay_s, is_lit};
use crate::environment::EnvironmentClock;
use crate::geometry::lamps::LampPost;
use crate::geometry::{mesh_cell_for, TileGeometry, GRASS_RGB};
use crate::home_store;
use crate::render::far_lights::{rss_mb, DistantLights};
use crate::render::lamps::{head_position, lamp_node, LampLights, LampMaterials, LAMP_POOL};
use crate::render::world_map::WorldMapView;
use crate::render::{building_material, material, solid_body, spawn_car, terrain_body};
use crate::world::far_lights::{spawn_load, FarLoad};
use crate::world::lod::{self, TerrainLod};
use crate::world::spawn::{find_spawn, find_spawn_near, inside_loaded_map};
use crate::world::streaming::{distance_to_tile, is_stale, tiles_in_range};
use crate::world::{open_map, MapIndex};
use tilebake::dem::MapGrid;
use tileformat::units::lamps::LAMP_REAL_REACH_M;

/// Height of the car above the terrain when spawned, in meters.
const SPAWN_LIFT_M: f32 = 1.0;
const TERRAIN_COLOR: (f32, f32, f32) = (GRASS_RGB[0], GRASS_RGB[1], GRASS_RGB[2]);
const ROAD_COLOR: (f32, f32, f32) = (0.22, 0.22, 0.24);
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

/// Streams `<cache_root>/<map_id>` around the car.
#[derive(GodotClass)]
#[class(base = Node3D, init)]
pub struct EarthWorld {
    base: Base<Node3D>,
    /// Folder name of the map under the cache root (the id given to tilebake).
    #[init(val = GString::from("casa"))]
    #[export]
    map_id: GString,
    /// Cache root. `res://` paths are resolved to the file system.
    #[init(val = GString::from("res://../cache"))]
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
    terrain_mat: Option<Gd<StandardMaterial3D>>,
    road_mat: Option<Gd<StandardMaterial3D>>,
    /// White albedo: meshes with per-vertex colors (buildings, terrain with ground cover) show
    /// their colors as they are.
    vertex_mat: Option<Gd<StandardMaterial3D>>,
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

    /// Worst `process` call and worst tile instantiation since the last log line, seconds.
    worst_frame: f64,
    worst_build: f64,
    stats_timer: f64,
}

#[godot_api]
impl EarthWorld {
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

#[godot_api]
impl INode3D for EarthWorld {
    fn ready(&mut self) {
        self.base_mut().set_process(true);
        let home = home_store::load();
        if let Some(h) = home {
            crate::environment::sky::set_home(h.lat, h.lon);
        }
        let id = self.map_id.to_string();
        let cache_root = PathBuf::from(
            ProjectSettings::singleton()
                .globalize_path(&self.cache_root)
                .to_string(),
        );
        let index = match open_map(&cache_root, &id) {
            Ok(index) => index,
            Err(e) => {
                godot_error!("cannot open map '{id}' from {}: {e}", cache_root.display());
                self.add_map_view(Some("home.no_map"));
                return;
            }
        };
        let tile_m = index.meta.tile_size_m;
        godot_print!(
            "map '{id}' opened: {} tiles, streamed within {} m",
            index.coords().len(),
            self.view_radius_m
        );
        self.terrain_mat = Some(material(TERRAIN_COLOR));
        self.road_mat = Some(material(ROAD_COLOR));
        self.vertex_mat = Some(material((1.0, 1.0, 1.0)));
        self.building_mat = building_material();
        if self.building_mat.is_none() {
            godot_warn!("building shader not loaded: buildings have no windows");
        }
        self.lamp_mats = LampMaterials::new();
        if self.lamp_mats.is_none() {
            godot_warn!("lamp shaders not loaded: street lamps are not drawn");
        }
        self.lamp_lights = Some(LampLights::new());
        if self.far_lights_enabled {
            let path = cache_root.join(&id).join("far_lights.bin");
            self.far_loader = Some(spawn_load(path, self.far_lights_max.max(0) as usize));
        }

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
                    return;
                }
            },
            None => None,
        };
        let picker_key = match (home, saved_pose) {
            (Some(_), Some(_)) => None,
            (Some(_), None) if saved_local.is_some() => Some("home.no_road_start"),
            (Some(_), None) => Some("home.outside_saved"),
            (None, _) => Some("home.none_start"),
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
                return;
            }
        };
        let spawn_tile = ((x / tile_m).floor() as i32, (y / tile_m).floor() as i32);
        let near = match index.neighborhood(spawn_tile.0, spawn_tile.1) {
            Ok(map) => map,
            Err(e) => {
                godot_error!("cannot read tiles at the spawn point of '{id}': {e}");
                return;
            }
        };
        let Some(height) = near.height_at(x, y) else {
            godot_error!("map '{id}' has no tile at the spawn point, car not spawned");
            return;
        };
        // The car lands on the spawn tile and its neighbors, so those are built now.
        for ty in spawn_tile.1 - 1..=spawn_tile.1 + 1 {
            for tx in spawn_tile.0 - 1..=spawn_tile.0 + 1 {
                self.load_now(&index, (tx, ty));
            }
        }

        let position = Vector3::new(x as f32, height + SPAWN_LIFT_M, -(y as f32));
        match spawn_car(position, yaw) {
            Some(car) => {
                self.base_mut().add_child(&car);
                self.car = Some(car);
            }
            None => {
                godot_error!("cannot instantiate res://car.tscn");
                return;
            }
        }
        self.add_environment();
        self.add_lamp_lights();
        if let (Some(car), Some(material)) = (&self.car, &self.terrain_mat) {
            lod::configure_view(self.base().get_parent(), car);
            self.lod = Some(TerrainLod::start(
                &cache_root,
                &id,
                &index.meta,
                material.clone(),
            ));
        }
        self.start_loader(Arc::new(index));
        self.add_map_view(picker_key);
    }

    fn process(&mut self, delta: f64) {
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
    /// Starts the loader thread. It builds tile geometry for each request, in order, and
    /// stops when this node (and with it the request sender) is freed.
    fn start_loader(&mut self, index: Arc<MapIndex>) {
        let (req_tx, req_rx) = mpsc::channel::<((i32, i32), f64)>();
        let (res_tx, res_rx) = mpsc::channel();
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
            // One mesh and one collision body per tile, for the buildings of that tile.
            let mut body = match &self.building_mat {
                Some(mat) => solid_body(&geometry.buildings, mat),
                None => solid_body(&geometry.buildings, &vertex_mat),
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
        let grid = self
            .index
            .as_ref()
            .map(|index| MapGrid::from_meta(&index.meta));
        let car = self.car.clone();
        let origin = self.to_gd().upcast::<Node3D>();
        let mut layer = CanvasLayer::new_alloc();
        layer.set_layer(MAP_CANVAS_LAYER);
        let mut map = WorldMapView::new_alloc();
        if let (Some(car), Some(grid)) = (car, grid) {
            map.bind_mut().start(car, origin, grid);
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
