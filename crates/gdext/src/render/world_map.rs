//! World map (F17b, F17c), scene `godot/home_picker.tscn` and the M map in the game. A full-screen
//! control: the mouse wheel zooms, dragging moves, a click sets the marker, and Confirm saves it
//! to `user://home.json`. In the game it starts hidden: M opens it (the game pauses, the mouse is
//! free, the view centers on the car at about 2 m per pixel), and M or Esc closes it. The data is
//! the embedded world asset (`res://assets/worldmap/world.bin`), read once; only the pieces on
//! screen are drawn, and coastline points closer than a pixel are dropped. The logic is in the
//! `worldmap` crate.
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use godot::classes::control::{LayoutPreset, MouseFilter};
use godot::classes::file_access::ModeFlags;
use godot::classes::input::MouseMode;
use godot::classes::node::ProcessMode;
use godot::classes::{
    Button, Control, FileAccess, IControl, Input, InputEvent, InputEventMouseButton,
    InputEventMouseMotion, Node3D, Os, ProjectSettings, RenderingServer, ThemeDb,
};
use godot::global::{HorizontalAlignment, MouseButton};
use godot::prelude::*;
use tilebake::dem::MapGrid;

use super::world_map_detail::{Loader, RETRY_AFTER};
use crate::home_store::{load as load_home, save as save_home};
use crate::ui_text::{language, text, Lang};
use worldmap::asset::{decode, WorldData};
use worldmap::cities::{label_width_px, place, reserve, Placed};
use worldmap::cull::{bbox_of, boxes_overlap, chunk_ranges};
use worldmap::home::Home;
use worldmap::labels::zoom_step;
use worldmap::ribbon::{local_width, Ribbon};
use worldmap::road_labels;
use worldmap::tiles::{
    cover, display_tiles, is_major_class, keep_set, zoom_for_view, Feature, TileData, TileId,
    MINOR_MIN_STEPS,
};
use worldmap::triangulate::ear_clip;
use worldmap::view::{zoom_steps_for_m_per_px, View, MAX_ZOOM_STEPS};

const MAP_PATH: &str = "res://assets/worldmap/world.bin";
/// Zoom when the map opens on the car, in meters per pixel.
const OPEN_M_PER_PX: f64 = 2.0;
/// Zoom factors of one wheel notch.
const ZOOM_IN: f64 = 1.25;
const ZOOM_OUT: f64 = 0.8;
/// A press that moves farther than this (pixels) is a drag, not a click.
const CLICK_MAX_PX: f64 = 4.0;
/// Coastline points closer than this (pixels) are dropped.
const MIN_POINT_PX: f64 = 0.5;
/// Polygons smaller than this (pixels, both sides) are not filled.
const MIN_FILL_PX: f64 = 3.0;
/// Zoom (doublings above the whole world) when a saved home is shown.
const HOME_ZOOM_STEPS: f64 = 4.0;
const FONT_PX: f64 = 13.0;
/// Tiles kept in memory before the ones off the view are dropped (read again from the disk cache).
const MAX_DETAIL_TILES: usize = 400;
const ROAD: Color = Color::from_rgb(0.62, 0.62, 0.60);
const ROAD_MAJOR: Color = Color::from_rgb(0.47, 0.47, 0.45);
const PLACE_DOT: Color = Color::from_rgb(0.25, 0.25, 0.25);
/// Neighborhood names drawn per frame at most.
const MAX_PLACE_LABELS: usize = 80;
const WATER: Color = Color::from_rgb(0.80, 0.87, 0.92);
const LAND: Color = Color::from_rgb(0.56, 0.74, 0.50);
const COAST: Color = Color::from_rgb(0.30, 0.42, 0.30);
const BORDER: Color = Color::from_rgb(0.55, 0.55, 0.52);
const CITY: Color = Color::from_rgb(0.15, 0.15, 0.15);
const LABEL: Color = Color::from_rgb(0.12, 0.12, 0.12);
const LABEL_OUTLINE: Color = Color::from_rgb(0.97, 0.97, 0.95);
const MARKER: Color = Color::from_rgb(0.85, 0.12, 0.12);
const CAR: Color = Color::from_rgb(0.85, 0.12, 0.12);
const HUD: Color = Color::from_rgb(0.10, 0.10, 0.10);

/// Dot radius by city tier (0 the largest).
const DOT_RADIUS: [f32; 4] = [4.0, 3.0, 2.5, 2.0];

/// Benchmark mode, for measuring (not for players): with this variable set, the picker opens at
/// the deepest zoom over downtown Belo Horizonte, pans slowly, prints the draw times and quits.
const BENCH_ENV: &str = "DTE_WORLDMAP_BENCH";
const BENCH_LAT: f64 = -19.92;
const BENCH_LON: f64 = -43.94;
const BENCH_WARMUP: u32 = 150;
const BENCH_FRAMES: u32 = 300;

/// What one paint drew: roads, names, places, tiles drawn, tiles resident and covering tiles missing.
#[derive(Default, Clone, Copy)]
struct Counts {
    roads: usize,
    labels: usize,
    tiles: usize,
    resident: usize,
    missing: usize,
    items: usize,
}

/// Benchmark state: waits until the view's tiles are loaded (or the wait times out), then warms up
/// and measures.
#[derive(Default)]
struct Bench {
    started: Option<Instant>,
    settled: u32,
    ready: bool,
    timed_out: bool,
    wait_ms: f64,
    frames: u32,
    draw_ms: Vec<f64>,
    overlay_ms: Vec<f64>,
    counts: Counts,
}

/// A chunk of a coastline or border line, with its box in the unit square.
struct Piece {
    pts: Vec<[f64; 2]>,
    bbox: [f64; 4],
}

/// Geometry prepared once at load, so a frame transforms only what reaches the screen.
#[derive(Default)]
struct Shapes {
    /// Per ring: its triangles (indices into the ring's points).
    tris: Vec<Vec<[u32; 3]>>,
    /// Per ring: the unit square box of each triangle.
    tri_boxes: Vec<Vec<[f64; 4]>>,
    coast: Vec<Piece>,
    border: Vec<Piece>,
}

/// Points per chunk of a coastline or border line.
const CHUNK_POINTS: usize = 64;
const BENCH_WAIT: Duration = Duration::from_secs(20);
/// Frames in a row with every covering tile loaded before the bench starts measuring.
const BENCH_SETTLE: u32 = 3;

impl Shapes {
    fn build(world: &WorldData) -> Shapes {
        let tris: Vec<Vec<[u32; 3]>> = world.rings.iter().map(|r| ear_clip(&r.pts)).collect();
        let tri_boxes = world
            .rings
            .iter()
            .zip(&tris)
            .map(|(ring, tr)| {
                tr.iter()
                    .map(|t| bbox_of(&t.map(|v| ring.pts[v as usize])))
                    .collect()
            })
            .collect();
        let mut coast = Vec::new();
        for ring in &world.rings {
            let mut pts = ring.pts.clone();
            if pts.len() >= 2 && pts.first() != pts.last() {
                pts.push(pts[0]);
            }
            push_chunks(&mut coast, &pts);
        }
        let mut border = Vec::new();
        for line in &world.lines {
            push_chunks(&mut border, &line.pts);
        }
        Shapes {
            tris,
            tri_boxes,
            coast,
            border,
        }
    }
}

fn push_chunks(out: &mut Vec<Piece>, pts: &[[f64; 2]]) {
    for (s, e) in chunk_ranges(pts.len(), CHUNK_POINTS) {
        let part = pts[s..e].to_vec();
        let bbox = bbox_of(&part);
        out.push(Piece { pts: part, bbox });
    }
}

#[derive(GodotClass)]
#[class(base = Control)]
pub struct WorldMapView {
    base: Base<Control>,
    world: Option<Arc<WorldData>>,
    /// Triangles, their boxes and line chunks, computed once at load.
    shapes: Arc<Shapes>,
    view: Option<View>,
    lang: Lang,
    /// Home chosen or saved, shown as the marker.
    home: Option<Home>,
    /// Status line, already in the player's language.
    status: String,
    /// Mouse position of a left press, until it is a click or a drag.
    press: Option<Vector2>,
    dragging: bool,
    confirm: Option<Gd<Button>>,
    last_size: Vector2,
    /// Vector tiles (roads, names) per tile, filled by the loader threads (F17b3).
    loader: Option<Loader>,
    details: Arc<HashMap<TileId, TileData>>,
    failed: HashMap<TileId, Instant>,
    /// A tile request failed (no network): a discreet message is shown.
    offline: bool,
    /// View position used for the last wanted list.
    view_key: Option<(f64, f64, f64)>,
    /// Covering tiles of the view not loaded yet (or failed), from the last wanted list.
    wanted_left: usize,
    /// Names, cities and texts, drawn above the roads (see `paint_top`).
    overlay: Option<Gd<Control>>,
    /// Road items per displayed tile (F17b).
    roads: HashMap<TileId, TileRoads>,
    /// Tiles drawn for the current view.
    shown: Vec<TileId>,
    bench: Option<Bench>,
    /// The car and the node whose frame is the map's (the earth world), set by `start` in the game.
    car: Option<Gd<Node3D>>,
    origin: Option<Gd<Node3D>>,
    grid: Option<MapGrid>,
    /// The map is open (M). Only then is it shown, processed and the game paused.
    open: bool,
    /// Mouse mode before the map opened, restored when it closes.
    mouse_before: Option<MouseMode>,
    /// Center on the car at the next frame with a view (set on open).
    focus_pending: bool,
}

#[godot_api]
impl IControl for WorldMapView {
    fn init(base: Base<Control>) -> Self {
        WorldMapView {
            base,
            world: None,
            shapes: Arc::new(Shapes::default()),
            view: None,
            lang: Lang::En,
            home: None,
            status: String::new(),
            press: None,
            dragging: false,
            confirm: None,
            last_size: Vector2::ZERO,
            loader: None,
            details: Arc::new(HashMap::new()),
            failed: HashMap::new(),
            offline: false,
            view_key: None,
            wanted_left: 0,
            overlay: None,
            roads: HashMap::new(),
            shown: Vec::new(),
            bench: None,
            car: None,
            origin: None,
            grid: None,
            open: false,
            mouse_before: None,
            focus_pending: false,
        }
    }

    fn ready(&mut self) {
        // Input and drawing must run while the tree is paused (M pauses the game).
        self.base_mut().set_process_mode(ProcessMode::ALWAYS);
        self.base_mut()
            .set_anchors_and_offsets_preset(LayoutPreset::FULL_RECT);
        self.base_mut().set_mouse_filter(MouseFilter::STOP);
        // Hidden and idle until M opens it.
        self.base_mut().set_visible(false);
        self.base_mut().set_process(false);
        if std::env::var_os(BENCH_ENV).is_some() {
            self.bench = Some(Bench {
                started: Some(Instant::now()),
                ..Bench::default()
            });
        }
        let locale = Os::singleton().get_locale().to_string();
        self.lang = language(&locale);
        self.world = load_world().map(Arc::new);
        self.loader = Some(Loader::start(cache_dir()));
        if let Some(world) = self.world.as_ref() {
            self.shapes = Arc::new(Shapes::build(world));
        }
        if self.world.is_none() {
            self.status = self.t("home.map_missing");
        }
        self.home = load_home();
        if let Some(h) = self.home {
            self.status = self.fmt("home.current", h);
        } else if self.world.is_some() {
            self.status = self.t("home.pick_first");
        }
        let mut overlay = Control::new_alloc();
        overlay.set_anchors_and_offsets_preset(LayoutPreset::FULL_RECT);
        overlay.set_mouse_filter(MouseFilter::IGNORE);
        overlay.set_z_index(2);
        let draw_handler = self.to_gd().callable("on_overlay_draw");
        overlay.connect("draw", &draw_handler);
        self.base_mut().add_child(&overlay);
        self.overlay = Some(overlay);
        let mut button = Button::new_alloc();
        button.set_text(&self.t("home.confirm"));
        button.set_custom_minimum_size(Vector2::new(160.0, 40.0));
        button.set_size(Vector2::new(160.0, 40.0));
        button.set_z_index(3);
        self.base_mut().add_child(&button);
        let handler = self.to_gd().callable("on_confirm");
        button.connect("pressed", &handler);
        self.confirm = Some(button);
    }

    fn exit_tree(&mut self) {
        let mut rs = RenderingServer::singleton();
        for (_, tr) in self.roads.drain() {
            for rid in tr.rids {
                rs.free_rid(rid);
            }
        }
    }

    fn process(&mut self, _delta: f64) {
        self.bench_step();
        self.pump_detail();
        if self.focus_pending {
            self.focus_car();
        }
        let size = self.base().get_size();
        if size.x <= 0.0 || size.y <= 0.0 || size == self.last_size {
            return;
        }
        self.last_size = size;
        let (w, h) = (size.x as f64, size.y as f64);
        match self.view.as_mut() {
            Some(v) => {
                v.width = w;
                v.height = h;
            }
            None => {
                let mut v = View::world(w, h);
                if self.bench.is_some() {
                    v.center_on(BENCH_LAT, BENCH_LON, MAX_ZOOM_STEPS);
                } else if let Some(home) = self.home {
                    v.center_on(home.lat, home.lon, HOME_ZOOM_STEPS);
                }
                self.view = Some(v);
            }
        }
        if let Some(button) = self.confirm.as_mut() {
            button.set_position(Vector2::new(size.x - 176.0, size.y - 56.0));
        }
        self.redraw();
    }

    fn unhandled_input(&mut self, event: Gd<InputEvent>) {
        if event.is_action_pressed("map_toggle") {
            self.set_open(!self.open);
        } else if self.open && event.is_action_pressed("ui_cancel") {
            self.set_open(false);
            // The car's own Esc (mouse mode) must not also run.
            if let Some(mut viewport) = self.base().get_viewport() {
                viewport.set_input_as_handled();
            }
        }
    }

    fn gui_input(&mut self, event: Gd<InputEvent>) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        if let Ok(button) = event.clone().try_cast::<InputEventMouseButton>() {
            let pos = button.get_position();
            let (sx, sy) = (pos.x as f64, pos.y as f64);
            match button.get_button_index() {
                MouseButton::WHEEL_UP if button.is_pressed() => view.zoom_at(sx, sy, ZOOM_IN),
                MouseButton::WHEEL_DOWN if button.is_pressed() => view.zoom_at(sx, sy, ZOOM_OUT),
                MouseButton::LEFT if button.is_pressed() => {
                    self.press = Some(pos);
                    self.dragging = false;
                }
                MouseButton::LEFT => {
                    let was_click = self.press.is_some() && !self.dragging;
                    self.press = None;
                    if was_click {
                        let (lat, lon) = view.lat_lon_at(sx, sy);
                        if let Ok(home) = Home::new(lat, lon) {
                            self.home = Some(home);
                            self.status = self.fmt("home.selected", home);
                        }
                    }
                }
                _ => return,
            }
        } else if let Ok(motion) = event.try_cast::<InputEventMouseMotion>() {
            let Some(from) = self.press else {
                return;
            };
            let pos = motion.get_position();
            if (pos - from).length() as f64 <= CLICK_MAX_PX && !self.dragging {
                return;
            }
            self.dragging = true;
            let rel = motion.get_relative();
            view.pan(rel.x as f64, rel.y as f64);
        } else {
            return;
        }
        self.redraw();
    }

    fn draw(&mut self) {
        let t0 = Instant::now();
        let counts = self.paint();
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        if let Some(b) = self.bench.as_mut() {
            if b.ready {
                b.frames += 1;
                if b.frames > BENCH_WARMUP {
                    b.draw_ms.push(ms);
                    b.counts = Counts {
                        labels: b.counts.labels,
                        ..counts
                    };
                }
            }
        }
    }
}

impl WorldMapView {
    /// Hands the map the car, the node whose frame is the map's, and the grid from that frame to
    /// lat/lon. Call before the node enters the tree (the game's M map).
    pub(crate) fn start(&mut self, car: Gd<Node3D>, origin: Gd<Node3D>, grid: MapGrid) {
        self.car = Some(car);
        self.origin = Some(origin);
        self.grid = Some(grid);
    }

    /// Opens or closes the map. Opening pauses the game, frees the mouse and centers on the car.
    /// Opens the map at start with a message (the game has no home or the home is not loaded).
    pub(crate) fn open_automatically(&mut self, key: &str) {
        self.status = self.t(key);
        self.set_open(true);
    }

    fn set_open(&mut self, open: bool) {
        if open == self.open {
            return;
        }
        self.open = open;
        self.press = None;
        self.dragging = false;
        self.base_mut().set_visible(open);
        self.base_mut().set_process(open);
        let mut tree = self.base().get_tree();
        tree.set_pause(open);
        let mut input = Input::singleton();
        if open {
            self.mouse_before = Some(input.get_mouse_mode());
            input.set_mouse_mode(MouseMode::VISIBLE);
            self.focus_pending = true;
            self.focus_car();
        } else if let Some(mode) = self.mouse_before.take() {
            input.set_mouse_mode(mode);
        }
        self.redraw();
    }

    /// Centers the view on the car at `OPEN_M_PER_PX`. Waits for the view when it does not exist
    /// yet (its size comes with the first frame).
    fn focus_car(&mut self) {
        let fix = self.car_fix();
        let Some(view) = self.view.as_mut() else {
            return;
        };
        self.focus_pending = false;
        if let Some((lat, lon, _)) = fix {
            let steps = zoom_steps_for_m_per_px(lat, OPEN_M_PER_PX, view.min_scale());
            view.center_on(lat, lon, steps);
        }
        self.redraw();
    }

    /// The car's place (lat, lon) and its heading (unit vector, east and north), if the car is set.
    fn car_fix(&self) -> Option<(f64, f64, [f64; 2])> {
        let car = self.car.as_ref()?;
        let origin = self.origin.as_ref()?;
        let grid = self.grid?;
        let local = origin.to_local(car.get_global_position());
        let (lat, lon) = grid.latlon(local.x as f64, -local.z as f64);
        // The car drives along its -Z axis; in the map (x east, y north = -z) that is (-z.x, z.z),
        // where z is the car's own +Z axis (the basis column).
        let basis = origin.get_global_basis().inverse() * car.get_global_basis();
        let z = basis.col_c();
        let (hx, hy) = (-(z.x as f64), z.z as f64);
        let n = hx.hypot(hy).max(1e-9);
        Some((lat, lon, [hx / n, hy / n]))
    }

    /// Benchmark: waits until every covering tile is loaded (or the wait times out), then warms up,
    /// pans slowly, and when done prints the draw times and quits.
    fn bench_step(&mut self) {
        let loaded = self.view.is_some() && self.wanted_left == 0;
        let Some(b) = self.bench.as_mut() else {
            return;
        };
        if !b.ready {
            b.settled = if loaded { b.settled + 1 } else { 0 };
            let waited = b.started.map(|t| t.elapsed()).unwrap_or_default();
            if b.settled >= BENCH_SETTLE || waited >= BENCH_WAIT {
                b.ready = true;
                b.timed_out = b.settled < BENCH_SETTLE;
                b.wait_ms = waited.as_secs_f64() * 1000.0;
            }
            self.redraw();
            return;
        }
        let frame = b.frames;
        if frame == BENCH_WARMUP + BENCH_FRAMES {
            let (avg, p95, max) = stats(&b.draw_ms);
            let (ov_avg, ov_p95, ov_max) = stats(&b.overlay_ms);
            let c = b.counts;
            let (wait_ms, timed_out) = (b.wait_ms, b.timed_out);
            godot_print!(
                "WORLDMAP_BENCH frames={} wait_ms={wait_ms:.0} timed_out={timed_out} draw_avg_ms={avg:.3} draw_p95_ms={p95:.3} draw_max_ms={max:.3} overlay_avg_ms={ov_avg:.3} overlay_p95_ms={ov_p95:.3} overlay_max_ms={ov_max:.3} tiles={} resident={} missing={} roads={} labels={} items={}",
                b.draw_ms.len(), c.tiles, c.resident, c.missing, c.roads, c.labels, c.items
            );
            self.base().get_tree().quit();
            return;
        }
        if frame > BENCH_WARMUP {
            if let Some(v) = self.view.as_mut() {
                v.pan(1.5, 0.7);
            }
        }
        self.redraw();
    }

    fn paint(&mut self) -> Counts {
        let size = self.base().get_size();
        self.base_mut()
            .draw_rect(Rect2::new(Vector2::ZERO, size), WATER);
        let details = self.details.clone();
        let mut counts = Counts {
            resident: details.len(),
            ..Counts::default()
        };
        // The tiles this view draws: its covering tiles, or their loaded parents.
        let mut ids: Vec<TileId> = Vec::new();
        if let Some(v) = self.view {
            if let Some(z) = zoom_for_view(&v) {
                ids = display_tiles(&v, z, |t| details.contains_key(t));
                counts.missing = cover(&v, z)
                    .iter()
                    .filter(|t| !details.contains_key(t))
                    .count();
            }
        }
        counts.tiles = ids.len();
        let parent = self.base().get_canvas_item();
        let sync = sync_roads(&mut self.roads, parent, &details, &ids, self.view.as_ref());
        counts.roads = sync.roads;
        counts.items = sync.items;
        if let (Some(world), Some(view)) = (self.world.clone(), self.view) {
            let shapes = self.shapes.clone();
            let mut ctrl = self.base_mut();
            draw_base(&mut ctrl, &world, &shapes, &view);
        }
        self.shown = ids;
        counts
    }

    /// Asks for a redraw of the base layer and of the overlay above the roads.
    fn redraw(&mut self) {
        self.base_mut().queue_redraw();
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.queue_redraw();
        }
    }

    /// Names, cities and the home marker: drawn above the roads. Returns the names drawn.
    fn paint_top(&self, ctrl: &mut Gd<Control>, font: &Gd<godot::classes::Font>) -> usize {
        let (Some(world), Some(view)) = (self.world.clone(), self.view) else {
            return 0;
        };
        let details = self.details.clone();
        let tiles: Vec<&TileData> = self.shown.iter().filter_map(|t| details.get(t)).collect();
        let placed = place(&world.cities, &view, FONT_PX);
        let mut boxes: Vec<[f64; 4]> = placed.iter().filter_map(|p| p.label).collect();
        let labels = draw_names(ctrl, &tiles, &view, font, &mut boxes).labels;
        for Placed { city, x, y, label } in placed {
            let c = &world.cities[city];
            let pos = Vector2::new(x as f32, y as f32);
            ctrl.draw_circle(pos, DOT_RADIUS[c.tier.min(3) as usize], CITY);
            if let Some(b) = label {
                let at = Vector2::new(b[0] as f32, (b[1] + FONT_PX * 0.8) as f32);
                let name = GString::from(c.name.as_str());
                ctrl.draw_string_ex(font, at + Vector2::new(1.0, 1.0), &name)
                    .font_size(FONT_PX as i32)
                    .modulate(LABEL_OUTLINE)
                    .done();
                ctrl.draw_string_ex(font, at, &name)
                    .font_size(FONT_PX as i32)
                    .modulate(LABEL)
                    .done();
            }
        }
        if let Some(home) = self.home {
            let (x, y) = view.screen_of(home.lat, home.lon);
            let pos = Vector2::new(x as f32, y as f32);
            ctrl.draw_circle(pos, 7.0, MARKER);
            ctrl.draw_circle(pos, 2.5, Color::from_rgb(1.0, 1.0, 1.0));
        }
        if let Some((lat, lon, heading)) = self.car_fix() {
            let (x, y) = view.screen_of(lat, lon);
            draw_car(ctrl, [x, y], heading);
        }
        labels
    }
}

#[godot_api]
impl WorldMapView {
    #[func]
    fn on_confirm(&mut self) {
        let Some(home) = self.home else {
            self.status = self.t("home.pick_first");
            self.redraw();
            return;
        };
        if save_home(home).is_err() {
            self.status = self.t("home.save_error");
            self.redraw();
            return;
        }
        crate::environment::sky::set_home(home.lat, home.lon);
        let saved = self.fmt("home.saved", home);
        match self.move_car(home) {
            // No car (the map alone): only the save.
            None => self.status = saved,
            // The car is on the road of the chosen place: the map closes.
            Some(true) => {
                self.status = saved;
                self.set_open(false);
            }
            // The place has no baked map yet: the car stays where it is.
            Some(false) => self.status = format!("{saved} {}", self.t("home.outside_map")),
        }
        self.redraw();
    }

    /// Moves the car to the chosen place (F17c). `None` when there is no car; `Some(false)` when
    /// the place is not inside the loaded map (or has no road), so the car did not move.
    fn move_car(&self, home: Home) -> Option<bool> {
        let (Some(_), Some(origin)) = (self.car.as_ref(), self.origin.as_ref()) else {
            return None;
        };
        let args = [home.lat.to_variant(), home.lon.to_variant()];
        let moved = origin
            .clone()
            .call("move_car_to_latlon", &args)
            .try_to::<bool>()
            .unwrap_or(false);
        Some(moved)
    }

    /// Draws the overlay above the roads: names, cities, the home marker and the texts.
    #[func]
    fn on_overlay_draw(&mut self) {
        let Some(mut overlay) = self.overlay.clone() else {
            return;
        };
        let t0 = Instant::now();
        let size = overlay.get_size();
        let Some(font) = ThemeDb::singleton().get_fallback_font() else {
            return;
        };
        let labels = self.paint_top(&mut overlay, &font);
        let show_detail = self.view.is_some_and(|v| zoom_for_view(&v).is_some());
        let offline_msg = (self.offline && show_detail).then(|| self.t("home.detail_offline"));
        let status = self.status.clone();
        // In the game (the car is set) the hint says how to close the map.
        let hint = self.t(if self.car.is_some() {
            "worldmap.hint"
        } else {
            "home.hint"
        });
        let attribution = self.t("home.attribution");
        overlay
            .draw_string_ex(
                &font,
                Vector2::new(16.0, 24.0),
                &GString::from(hint.as_str()),
            )
            .font_size(14)
            .modulate(HUD)
            .done();
        if let Some(msg) = offline_msg {
            overlay
                .draw_string_ex(
                    &font,
                    Vector2::new(16.0, 46.0),
                    &GString::from(msg.as_str()),
                )
                .font_size(12)
                .modulate(HUD)
                .done();
        }
        overlay
            .draw_string_ex(
                &font,
                Vector2::new(16.0, size.y - 60.0),
                &GString::from(status.as_str()),
            )
            .font_size(16)
            .modulate(HUD)
            .done();
        overlay
            .draw_string_ex(
                &font,
                Vector2::new(16.0, size.y - 16.0),
                &GString::from(attribution.as_str()),
            )
            .font_size(12)
            .modulate(HUD)
            .done();
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        if let Some(b) = self.bench.as_mut() {
            if b.ready && b.frames > BENCH_WARMUP {
                b.overlay_ms.push(ms);
                b.counts.labels = labels;
            }
        }
    }

    /// Takes the tiles the loader finished, and asks for the tiles of the view when it moved.
    fn pump_detail(&mut self) {
        let Some(loader) = self.loader.as_ref() else {
            return;
        };
        let done = loader.drain();
        let got_any = !done.is_empty();
        if got_any {
            let details = Arc::make_mut(&mut self.details);
            for (tile, result) in done {
                match result {
                    Ok(detail) => {
                        details.insert(tile, detail);
                        self.failed.remove(&tile);
                        self.offline = false;
                    }
                    Err(_) => {
                        self.failed.insert(tile, Instant::now());
                        self.offline = true;
                    }
                }
            }
        }
        let Some(view) = self.view.as_ref() else {
            return;
        };
        let key = (view.cx, view.cy, view.px_per_unit);
        if self.view_key == Some(key) && !got_any {
            return;
        }
        self.view_key = Some(key);
        let now = Instant::now();
        let zoom = zoom_for_view(view);
        if self.details.len() > MAX_DETAIL_TILES {
            // Only the tiles around the view stay; the others are read again from the disk cache.
            let keep = zoom.map(|z| keep_set(view, z)).unwrap_or_default();
            Arc::make_mut(&mut self.details).retain(|t, _| keep.contains(t));
        }
        // Nearest first (`cover` sorts), skipping tiles we have or failed for a while.
        let wanted: Vec<TileId> = match zoom {
            None => Vec::new(),
            Some(z) => cover(view, z)
                .into_iter()
                .filter(|t| !self.details.contains_key(t))
                .filter(|t| {
                    !self
                        .failed
                        .get(t)
                        .is_some_and(|at| now.duration_since(*at) < RETRY_AFTER)
                })
                .collect(),
        };
        self.wanted_left = wanted.len();
        loader.want(wanted);
        if got_any {
            self.redraw();
        }
    }

    fn t(&self, key: &str) -> String {
        text(key, self.lang).to_string()
    }

    /// A text with `{lat}` and `{lon}` filled from a home (4 decimals, about 10 m).
    fn fmt(&self, key: &str, home: Home) -> String {
        text(key, self.lang)
            .replace("{lat}", &format!("{:.4}", home.lat))
            .replace("{lon}", &format!("{:.4}", home.lon))
    }
}

fn load_world() -> Option<WorldData> {
    let mut file = FileAccess::open(MAP_PATH, ModeFlags::READ)?;
    let len = file.get_length() as i64;
    let bytes = file.get_buffer(len);
    decode(bytes.as_slice()).ok()
}

/// Disk cache of streets and neighborhoods, as an absolute path (the thread does plain file I/O).
fn cache_dir() -> PathBuf {
    PathBuf::from(
        ProjectSettings::singleton()
            .globalize_path("user://worldmap_cache")
            .to_string(),
    )
}

/// A ring whose box is under 3 pixels on screen is not drawn (it is invisible, and the fill rejects some).
fn too_small(b: [f64; 4], view: &View) -> bool {
    (b[2] - b[0]) * view.px_per_unit < MIN_FILL_PX && (b[3] - b[1]) * view.px_per_unit < MIN_FILL_PX
}

fn to_points(pts: &[[f64; 2]]) -> PackedVector2Array {
    pts.iter()
        .map(|p| Vector2::new(p[0] as f32, p[1] as f32))
        .collect()
}

/// Fills the rings of one kind (land or water holes) at copy `k`. Only the triangles whose box
/// reaches the screen are sent, transformed exactly (no pixel dropping, which would break them).
fn fill_rings(
    ctrl: &mut Gd<Control>,
    world: &WorldData,
    shapes: &Shapes,
    view: &View,
    k: i32,
    hole: bool,
    color: Color,
) {
    let item = ctrl.get_canvas_item();
    // The screen in unit coordinates, moved back by the copy shift.
    let (ax, ay) = view.to_unit(0.0, 0.0);
    let (bx, by) = view.to_unit(view.width, view.height);
    let window = [
        ax.min(bx) - k as f64,
        ay.min(by),
        ax.max(bx) - k as f64,
        ay.max(by),
    ];
    for (i, ring) in world
        .rings
        .iter()
        .enumerate()
        .filter(|(_, r)| r.hole == hole)
    {
        if shapes.tris[i].is_empty()
            || !view.box_visible(k, ring.bbox)
            || too_small(ring.bbox, view)
        {
            continue;
        }
        let mut points: Vec<Vector2> = Vec::new();
        for (t, tb) in shapes.tris[i].iter().zip(&shapes.tri_boxes[i]) {
            if !boxes_overlap(*tb, window) {
                continue;
            }
            for &v in t {
                let p = ring.pts[v as usize];
                let (x, y) = view.to_screen(p[0] + k as f64, p[1]);
                points.push(Vector2::new(x as f32, y as f32));
            }
        }
        if points.is_empty() {
            continue;
        }
        let indices: PackedInt32Array = (0..points.len() as i32).collect();
        let colors: PackedColorArray = std::iter::repeat(color).take(points.len()).collect();
        let points: PackedVector2Array = points.into_iter().collect();
        RenderingServer::singleton()
            .canvas_item_add_triangle_array(item, &indices, &points, &colors);
    }
}

/// Draws the chunks of coastline or border lines whose box reaches the screen, at copy `k`.
fn draw_pieces(ctrl: &mut Gd<Control>, pieces: &[Piece], view: &View, k: i32, color: Color) {
    for piece in pieces.iter().filter(|p| view.box_visible(k, p.bbox)) {
        let pts = view.screen_points(&piece.pts, k, MIN_POINT_PX);
        if pts.len() >= 2 {
            ctrl.draw_polyline_ex(&to_points(&pts), color)
                .width(1.0)
                .done();
        }
    }
}

/// Draws the water inside the land, the land, coastlines and borders: the base layer under the roads.
fn draw_base(ctrl: &mut Gd<Control>, world: &WorldData, shapes: &Shapes, view: &View) {
    let copies = view.copies();
    for &k in &copies {
        fill_rings(ctrl, world, shapes, view, k, false, LAND);
    }
    for &k in &copies {
        fill_rings(ctrl, world, shapes, view, k, true, WATER);
        draw_pieces(ctrl, &shapes.coast, view, k, COAST);
        draw_pieces(ctrl, &shapes.border, view, k, BORDER);
    }
}

/// Road buckets, drawn in this order (minor roads first, then the major ones): width in pixels at
/// the start of a zoom step, and colour.
const ROAD_BUCKETS: [(f64, Color); 4] = [
    (1.2, ROAD),
    (2.0, ROAD),
    (2.6, ROAD_MAJOR),
    (3.5, ROAD_MAJOR),
];
/// Tiles whose road items are rebuilt per frame at most (after a zoom step change).
const ROAD_BUILDS_PER_FRAME: usize = 4;

/// Bucket of a road class: 3 motorway and trunk, 2 primary and secondary, 1 tertiary, 0 the rest.
fn road_bucket(class: &str) -> usize {
    match class {
        "motorway" | "trunk" => 3,
        "primary" | "secondary" => 2,
        "tertiary" => 1,
        _ => 0,
    }
}

/// The road items of one tile: one canvas item per bucket, built for one zoom step. Their points
/// are in tile-local units (the tile is the unit square), so panning and zooming only move them.
struct TileRoads {
    step: i32,
    rids: [Rid; ROAD_BUCKETS.len()],
}

/// Road features drawn and canvas items kept, for the bench.
struct RoadSync {
    roads: usize,
    items: usize,
}

/// Ribbons of one tile for a zoom step, one per bucket, in tile-local units.
fn build_roads(data: &TileData, id: TileId, step: i32) -> [Ribbon; ROAD_BUCKETS.len()] {
    let scale = 2f64.powi(id.z as i32);
    let mut out: [Ribbon; ROAD_BUCKETS.len()] = Default::default();
    for road in &data.roads {
        let b = road_bucket(&road.class);
        let pts: Vec<[f64; 2]> = road
            .pts
            .iter()
            .map(|p| [p[0] * scale - id.x as f64, p[1] * scale - id.y as f64])
            .collect();
        out[b].push_polyline(&pts, local_width(ROAD_BUCKETS[b].0, step, id.z));
    }
    out
}

/// Keeps the road items of the displayed tiles (F17b). Items of tiles no longer displayed are
/// freed. A tile built for another zoom step is rebuilt, at most `ROAD_BUILDS_PER_FRAME` per call,
/// and keeps its old items until then. Every displayed tile gets its transform and visibility.
fn sync_roads(
    roads: &mut HashMap<TileId, TileRoads>,
    parent: Rid,
    details: &HashMap<TileId, TileData>,
    ids: &[TileId],
    view: Option<&View>,
) -> RoadSync {
    let mut rs = RenderingServer::singleton();
    let keep: HashSet<TileId> = ids.iter().copied().collect();
    roads.retain(|id, tr| {
        let kept = keep.contains(id);
        if !kept {
            for rid in tr.rids {
                rs.free_rid(rid);
            }
        }
        kept
    });
    let Some(view) = view else {
        return RoadSync {
            roads: 0,
            items: roads.len() * ROAD_BUCKETS.len(),
        };
    };
    let step = zoom_step(view.px_per_unit);
    let mut budget = ROAD_BUILDS_PER_FRAME;
    for id in ids {
        let Some(data) = details.get(id) else {
            continue;
        };
        let current = roads.get(id).is_some_and(|tr| tr.step == step);
        if current || budget == 0 {
            continue;
        }
        budget -= 1;
        let ribbons = build_roads(data, *id, step);
        if !roads.contains_key(id) {
            let rids: [Rid; ROAD_BUCKETS.len()] = std::array::from_fn(|b| {
                let rid = rs.canvas_item_create();
                rs.canvas_item_set_parent(rid, parent);
                rs.canvas_item_set_z_index(rid, if b == 0 { 0 } else { 1 });
                rid
            });
            roads.insert(*id, TileRoads { step, rids });
        }
        let Some(tr) = roads.get_mut(id) else {
            continue;
        };
        tr.step = step;
        for (b, ribbon) in ribbons.iter().enumerate() {
            let rid = tr.rids[b];
            rs.canvas_item_clear(rid);
            if ribbon.indices.is_empty() {
                continue;
            }
            let points: PackedVector2Array = ribbon
                .verts
                .iter()
                .map(|v| Vector2::new(v[0], v[1]))
                .collect();
            let indices: PackedInt32Array = ribbon.indices.iter().map(|&i| i as i32).collect();
            let colors: PackedColorArray = [ROAD_BUCKETS[b].1].into_iter().collect();
            rs.canvas_item_add_triangle_array_ex(rid, &indices, &points, &colors)
                .done();
        }
    }
    let mut out = RoadSync {
        roads: 0,
        items: roads.len() * ROAD_BUCKETS.len(),
    };
    for id in ids {
        let (Some(tr), Some(data)) = (roads.get(id), details.get(id)) else {
            continue;
        };
        let scale = 2f64.powi(id.z as i32);
        let s = (view.px_per_unit / scale) as f32;
        let origin = Vector2::new(
            ((id.x as f64 / scale - view.cx) * view.px_per_unit + view.width / 2.0) as f32,
            ((id.y as f64 / scale - view.cy) * view.px_per_unit + view.height / 2.0) as f32,
        );
        let transform = Transform2D::from_cols(Vector2::new(s, 0.0), Vector2::new(0.0, s), origin);
        let unit = [
            id.x as f64 / scale,
            id.y as f64 / scale,
            (id.x as f64 + 1.0) / scale,
            (id.y as f64 + 1.0) / scale,
        ];
        let visible = view.box_visible(0, unit);
        let minor = visible && view.zoom_steps() >= MINOR_MIN_STEPS;
        for (b, rid) in tr.rids.iter().enumerate() {
            rs.canvas_item_set_transform(*rid, transform);
            rs.canvas_item_set_visible(*rid, if b == 0 { minor } else { visible });
        }
        if visible {
            out.roads += data.roads.len();
        }
    }
    out
}

/// Average, 95th percentile and maximum of times in milliseconds.
/// The car as an arrow at screen point `c`, pointing along `heading` (east, north; north is up).
fn draw_car(ctrl: &mut Gd<Control>, c: [f64; 2], heading: [f64; 2]) {
    // Screen direction: x as is, y flipped.
    let d = (heading[0], -heading[1]);
    let p = (-d.1, d.0);
    let at = |f: f64, s: f64| {
        Vector2::new(
            (c[0] + d.0 * f + p.0 * s) as f32,
            (c[1] + d.1 * f + p.1 * s) as f32,
        )
    };
    let points = PackedVector2Array::from([at(14.0, 0.0), at(-8.0, 8.0), at(-8.0, -8.0)]);
    ctrl.draw_colored_polygon(&points, CAR);
}

fn stats(times: &[f64]) -> (f64, f64, f64) {
    let mut t = times.to_vec();
    let n = t.len().max(1) as f64;
    let avg = t.iter().sum::<f64>() / n;
    let max = t.iter().cloned().fold(0.0, f64::max);
    t.sort_by(|a, b| a.total_cmp(b));
    let p95 = t
        .get((t.len() as f64 * 0.95) as usize)
        .copied()
        .unwrap_or(0.0);
    (avg, p95, max)
}

/// Screen points of a tile feature and their bounding box `[x0, y0, x1, y1]`.
fn screen_line(f: &Feature, view: &View) -> (Vec<[f64; 2]>, [f64; 4]) {
    let pts: Vec<[f64; 2]> = f
        .pts
        .iter()
        .map(|p| {
            let (x, y) = view.to_screen(p[0], p[1]);
            [x, y]
        })
        .collect();
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for p in &pts {
        b = [
            b[0].min(p[0]),
            b[1].min(p[1]),
            b[2].max(p[0]),
            b[3].max(p[1]),
        ];
    }
    (pts, b)
}

/// Road names where they fit, and neighborhood names of the tiles drawn for this view. The roads
/// are the retained items of `sync_roads`. Minor streets' names only from `MINOR_MIN_STEPS`. Every
/// name is reserved in `boxes` (cities first), so names never overlap each other.
fn draw_names(
    ctrl: &mut Gd<Control>,
    tiles: &[&TileData],
    view: &View,
    font: &Gd<godot::classes::Font>,
    boxes: &mut Vec<[f64; 4]>,
) -> Counts {
    let mut counts = Counts::default();
    let (w, h) = (view.width, view.height);
    let minor = view.zoom_steps() >= MINOR_MIN_STEPS;
    let shown = |class: &str| is_major_class(class) || minor;
    for &data in tiles {
        for name in data
            .road_names
            .iter()
            .filter(|r| shown(&r.class) && view.box_visible(0, r.bbox))
        {
            let (pts, _) = screen_line(name, view);
            // Rotated to the road, repeated along long lines; only straight spots (road_labels).
            let size = if is_major_class(&name.class) {
                12.0
            } else {
                11.0
            };
            let text_w = label_width_px(&name.name, size);
            let label = GString::from(name.name.as_str());
            for lab in road_labels::place(&pts, text_w, size) {
                if !reserve(boxes, lab.aabb(), w) {
                    continue;
                }
                let pos = Vector2::new(lab.center[0] as f32, lab.center[1] as f32);
                ctrl.draw_set_transform_ex(pos)
                    .rotation(lab.angle as f32)
                    .done();
                let at = Vector2::new(0.0, (size * 0.35) as f32);
                ctrl.draw_string_ex(font, at + Vector2::new(1.0, 1.0), &label)
                    .alignment(HorizontalAlignment::CENTER)
                    .width(-1.0)
                    .font_size(size as i32)
                    .modulate(LABEL_OUTLINE)
                    .done();
                ctrl.draw_string_ex(font, at, &label)
                    .alignment(HorizontalAlignment::CENTER)
                    .width(-1.0)
                    .font_size(size as i32)
                    .modulate(LABEL)
                    .done();
                counts.labels += 1;
            }
            ctrl.draw_set_transform_ex(Vector2::ZERO)
                .rotation(0.0)
                .done();
        }
    }
    // Neighborhoods: larger classes first, so they win the room.
    let rank = |class: &str| match class {
        "quarter" => 0,
        "suburb" => 1,
        "neighbourhood" => 2,
        _ => 3,
    };
    let mut places: Vec<(u8, &Feature, [f64; 2])> = Vec::new();
    for &data in tiles {
        for place in &data.places {
            let (x, y) = view.to_screen(place.pts[0][0], place.pts[0][1]);
            if x >= 0.0 && y >= 0.0 && x <= w && y <= h {
                places.push((rank(&place.class), place, [x, y]));
            }
        }
    }
    places.sort_by_key(|(r, _, _)| *r);
    let mut drawn = 0;
    for (_, place, [x, y]) in places {
        if drawn >= MAX_PLACE_LABELS {
            break;
        }
        let text_w = label_width_px(&place.name, 12.0);
        let rect = [x + 5.0, y - 6.0, x + 5.0 + text_w, y + 6.0];
        if !reserve(boxes, rect, w) {
            continue;
        }
        let pos = Vector2::new(x as f32, y as f32);
        ctrl.draw_circle(pos, 2.0, PLACE_DOT);
        let label = GString::from(place.name.as_str());
        ctrl.draw_string_ex(font, pos + Vector2::new(5.0, 4.0), &label)
            .font_size(12)
            .modulate(LABEL_OUTLINE)
            .done();
        ctrl.draw_string_ex(font, pos + Vector2::new(4.0, 3.0), &label)
            .font_size(12)
            .modulate(LABEL)
            .done();
        drawn += 1;
        counts.labels += 1;
    }
    counts
}
