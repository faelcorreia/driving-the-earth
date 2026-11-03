//! World map for choosing the home place (F17b), scene `godot/home_picker.tscn`. A full-screen
//! control: the mouse wheel zooms, dragging moves, a click sets the marker, and Confirm saves it
//! to `user://home.json`. The data is the embedded world asset (`res://assets/worldmap/world.bin`),
//! read once; only the pieces on screen are drawn, and coastline points closer than a pixel are
//! dropped. The logic is in the `worldmap` crate. Not connected to the bake yet (F17c).
use std::sync::Arc;

use godot::classes::control::{LayoutPreset, MouseFilter};
use godot::classes::file_access::ModeFlags;
use godot::classes::{
    Button, Control, FileAccess, IControl, InputEvent, InputEventMouseButton,
    InputEventMouseMotion, Os, ThemeDb,
};
use godot::global::MouseButton;
use godot::prelude::*;

use crate::ui_text::{language, text, Lang};
use worldmap::asset::{decode, WorldData};
use worldmap::cities::{place, Placed};
use worldmap::home::{home_json, parse_home, Home};
use worldmap::view::View;

const MAP_PATH: &str = "res://assets/worldmap/world.bin";
const HOME_PATH: &str = "user://home.json";
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
const WATER: Color = Color::from_rgb(0.80, 0.87, 0.92);
const LAND: Color = Color::from_rgb(0.56, 0.74, 0.50);
const COAST: Color = Color::from_rgb(0.30, 0.42, 0.30);
const BORDER: Color = Color::from_rgb(0.55, 0.55, 0.52);
const CITY: Color = Color::from_rgb(0.15, 0.15, 0.15);
const LABEL: Color = Color::from_rgb(0.12, 0.12, 0.12);
const LABEL_OUTLINE: Color = Color::from_rgb(0.97, 0.97, 0.95);
const MARKER: Color = Color::from_rgb(0.85, 0.12, 0.12);
const HUD: Color = Color::from_rgb(0.10, 0.10, 0.10);

/// Dot radius by city tier (0 the largest).
const DOT_RADIUS: [f32; 4] = [4.0, 3.0, 2.5, 2.0];

#[derive(GodotClass)]
#[class(base = Control)]
pub struct WorldMapView {
    base: Base<Control>,
    world: Option<Arc<WorldData>>,
    /// Per ring: whether the polygon fill accepts it (a few rings touch themselves; they are only
    /// outlined).
    fillable: Vec<bool>,
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
}

#[godot_api]
impl IControl for WorldMapView {
    fn init(base: Base<Control>) -> Self {
        WorldMapView {
            base,
            world: None,
            fillable: Vec::new(),
            view: None,
            lang: Lang::En,
            home: None,
            status: String::new(),
            press: None,
            dragging: false,
            confirm: None,
            last_size: Vector2::ZERO,
        }
    }

    fn ready(&mut self) {
        self.base_mut()
            .set_anchors_and_offsets_preset(LayoutPreset::FULL_RECT);
        self.base_mut().set_mouse_filter(MouseFilter::STOP);
        self.base_mut().set_process(true);
        let locale = Os::singleton().get_locale().to_string();
        self.lang = language(&locale);
        self.world = load_world().map(Arc::new);
        if let Some(world) = self.world.as_ref() {
            self.fillable = fillable_rings(world);
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
        let mut button = Button::new_alloc();
        button.set_text(&self.t("home.confirm"));
        button.set_custom_minimum_size(Vector2::new(160.0, 40.0));
        button.set_size(Vector2::new(160.0, 40.0));
        self.base_mut().add_child(&button);
        let handler = self.to_gd().callable("on_confirm");
        button.connect("pressed", &handler);
        self.confirm = Some(button);
    }

    fn process(&mut self, _delta: f64) {
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
                if let Some(home) = self.home {
                    v.center_on(home.lat, home.lon, HOME_ZOOM_STEPS);
                }
                self.view = Some(v);
            }
        }
        if let Some(button) = self.confirm.as_mut() {
            button.set_position(Vector2::new(size.x - 176.0, size.y - 56.0));
        }
        self.base_mut().queue_redraw();
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
        self.base_mut().queue_redraw();
    }

    fn draw(&mut self) {
        let size = self.base().get_size();
        self.base_mut()
            .draw_rect(Rect2::new(Vector2::ZERO, size), WATER);
        let Some(font) = ThemeDb::singleton().get_fallback_font() else {
            return;
        };
        let world = self.world.clone();
        let fillable = self.fillable.clone();
        let view = self.view;
        let home = self.home;
        let status = self.status.clone();
        let attribution = self.t("home.attribution");
        let hint = self.t("home.hint");
        let mut ctrl = self.base_mut();
        if let (Some(world), Some(view)) = (world, view) {
            draw_map(&mut ctrl, &world, &fillable, &view, &font);
            if let Some(home) = home {
                let (x, y) = view.screen_of(home.lat, home.lon);
                let pos = Vector2::new(x as f32, y as f32);
                ctrl.draw_circle(pos, 7.0, MARKER);
                ctrl.draw_circle(pos, 2.5, Color::from_rgb(1.0, 1.0, 1.0));
            }
        }
        ctrl.draw_string_ex(&font, Vector2::new(16.0, 24.0), &GString::from(hint.as_str()))
            .font_size(14)
            .modulate(HUD)
            .done();
        ctrl.draw_string_ex(&font, Vector2::new(16.0, size.y - 60.0), &GString::from(status.as_str()))
            .font_size(16)
            .modulate(HUD)
            .done();
        ctrl.draw_string_ex(&font, Vector2::new(16.0, size.y - 16.0), &GString::from(attribution.as_str()))
            .font_size(12)
            .modulate(HUD)
            .done();
    }
}

#[godot_api]
impl WorldMapView {
    #[func]
    fn on_confirm(&mut self) {
        let msg = match self.home {
            None => self.t("home.pick_first"),
            Some(home) => match save_home(home) {
                Ok(()) => self.fmt("home.saved", home),
                Err(()) => self.t("home.save_error"),
            },
        };
        self.status = msg;
        self.base_mut().queue_redraw();
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

fn load_home() -> Option<Home> {
    if !FileAccess::file_exists(HOME_PATH) {
        return None;
    }
    let file = FileAccess::open(HOME_PATH, ModeFlags::READ)?;
    parse_home(&file.get_as_text().to_string()).ok()
}

fn save_home(home: Home) -> Result<(), ()> {
    let mut file = FileAccess::open(HOME_PATH, ModeFlags::WRITE).ok_or(())?;
    if file.store_string(&home_json(home)) {
        Ok(())
    } else {
        Err(())
    }
}

/// Whether Godot's triangulation accepts each ring, checked once on the unit-square shape.
fn fillable_rings(world: &WorldData) -> Vec<bool> {
    world
        .rings
        .iter()
        .map(|r| {
            let pts: Vec<[f64; 2]> = r.pts.iter().map(|p| [p[0] * 1000.0, p[1] * 1000.0]).collect();
            let arr = to_points(&fill_points(pts));
            godot::classes::Geometry2D::singleton()
                .triangulate_polygon(&arr)
                .len()
                > 0
        })
        .collect()
}

/// A polygon under 3 pixels on screen is not drawn (it is invisible, and the fill rejects some).
fn too_small(pts: &[[f64; 2]]) -> bool {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in pts {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    x1 - x0 < MIN_FILL_PX && y1 - y0 < MIN_FILL_PX
}

/// A closed ring without its repeated last point: Godot's polygon fill rejects the repeat.
fn fill_points(mut pts: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    if pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    pts
}

fn to_points(pts: &[[f64; 2]]) -> PackedVector2Array {
    pts.iter()
        .map(|p| Vector2::new(p[0] as f32, p[1] as f32))
        .collect()
}

/// Draws water pieces (inside land), land, coastlines, borders, cities and their names.
fn draw_map(
    ctrl: &mut Gd<Control>,
    world: &WorldData,
    fillable: &[bool],
    view: &View,
    font: &Gd<godot::classes::Font>,
) {
    let copies = view.copies();
    for &k in &copies {
        for (i, ring) in world.rings.iter().enumerate().filter(|(_, r)| !r.hole) {
            if fillable[i] && view.box_visible(k, ring.bbox) {
                let pts = fill_points(view.screen_points(&ring.pts, k, MIN_POINT_PX));
                if pts.len() >= 3 && !too_small(&pts) {
                    ctrl.draw_colored_polygon(&to_points(&pts), LAND);
                }
            }
        }
    }
    for &k in &copies {
        for (i, ring) in world.rings.iter().enumerate().filter(|(_, r)| r.hole) {
            if fillable[i] && view.box_visible(k, ring.bbox) {
                let pts = fill_points(view.screen_points(&ring.pts, k, MIN_POINT_PX));
                if pts.len() >= 3 && !too_small(&pts) {
                    ctrl.draw_colored_polygon(&to_points(&pts), WATER);
                }
            }
        }
        for ring in world.rings.iter() {
            if view.box_visible(k, ring.bbox) {
                let mut pts = view.screen_points(&ring.pts, k, MIN_POINT_PX);
                if pts.len() >= 2 {
                    if pts.first() != pts.last() {
                        pts.push(pts[0]);
                    }
                    ctrl.draw_polyline_ex(&to_points(&pts), COAST).width(1.0).done();
                }
            }
        }
        for line in &world.lines {
            if view.box_visible(k, line.bbox) {
                let pts = view.screen_points(&line.pts, k, MIN_POINT_PX);
                if pts.len() >= 2 {
                    ctrl.draw_polyline_ex(&to_points(&pts), BORDER).width(1.0).done();
                }
            }
        }
    }
    for Placed { city, x, y, label } in place(&world.cities, view, FONT_PX) {
        let c = &world.cities[city];
        let pos = Vector2::new(x as f32, y as f32);
        ctrl.draw_circle(pos, DOT_RADIUS[c.tier.min(3) as usize], CITY);
        if let Some(b) = label {
            let at = Vector2::new(b[0] as f32, (b[1] + FONT_PX * 0.8) as f32);
            let name = GString::from(c.name.as_str());
            ctrl.draw_string_ex(font, at + Vector2::new(1.0, 1.0), &name).font_size(FONT_PX as i32).modulate(LABEL_OUTLINE).done();
            ctrl.draw_string_ex(font, at, &name).font_size(FONT_PX as i32).modulate(LABEL).done();
        }
    }
}
