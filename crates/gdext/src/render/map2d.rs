//! The 2D map (F15a): a full-screen control toggled by the `map_toggle` action (M). While it is
//! open the game is paused; the mouse wheel zooms, dragging moves, and the car is drawn as an arrow
//! with its heading. Roads come from `world::map2d`, read in the background; only the visible
//! roads are drawn, batched by class, with points closer than a pixel dropped. Layer: `render`.
use std::sync::Arc;

use godot::classes::control::{LayoutPreset, MouseFilter};
use godot::classes::input::MouseMode;
use godot::classes::node::ProcessMode;
use godot::classes::{
    Control, Font, IControl, Input, InputEvent, InputEventMouseButton, InputEventMouseMotion,
    Node3D, Os, ThemeDb,
};
use godot::global::{HorizontalAlignment, MouseButton};
use godot::prelude::*;

use crate::ui_text::{language, text, Lang};
use crate::world::map::MapIndex;
use crate::world::map2d::{
    decimate, label_anchor, road_class, select_labels, shows_label, shows_road, spawn_load,
    stroke_px, LabelCandidate, MapLoad, MapRoads, View,
};

/// Zoom factor of one wheel notch.
const WHEEL_STEP: f64 = 0.8;
/// Zoom when the map opens, in meters per pixel.
const OPEN_M_PER_PX: f64 = 2.0;
/// Points closer than this on screen are dropped from a road.
const MIN_POINT_PX: f64 = 1.5;
/// A road shorter than this on screen gets no name.
const MIN_LABEL_PX: f64 = 60.0;
/// Named road pieces considered per frame at most (the nearest in the order of the tiles).
const MAX_LABEL_CANDIDATES: usize = 1500;
const BACKGROUND: Color = Color::from_rgb(0.56, 0.74, 0.50);
const ROAD: Color = Color::from_rgb(0.56, 0.56, 0.58);
const LABEL: Color = Color::from_rgb(0.12, 0.12, 0.12);
const LABEL_OUTLINE: Color = Color::from_rgb(0.97, 0.97, 0.95);
const CAR: Color = Color::from_rgb(0.85, 0.12, 0.12);
const TEXT: Color = Color::from_rgb(0.97, 0.97, 0.95);
const TEXT_SHADOW: Color = Color::from_rgb(0.1, 0.1, 0.1);

/// Full-screen 2D map. Add it under a `CanvasLayer` and call `start` once.
#[derive(GodotClass)]
#[class(base = Control)]
pub struct MapView {
    base: Base<Control>,
    loader: Option<MapLoad>,
    roads: Option<MapRoads>,
    load_failed: bool,
    car: Option<Gd<Node3D>>,
    /// Node whose local frame is the map's (the earth world).
    origin: Option<Gd<Node3D>>,
    view: Option<View>,
    lang: Option<Lang>,
    open: bool,
    /// Mouse position of the last drag event, while the left button is down.
    drag_from: Option<Vector2>,
    /// Mouse mode before the map opened, restored when it closes.
    mouse_before: Option<MouseMode>,
}

#[godot_api]
impl IControl for MapView {
    fn init(base: Base<Control>) -> Self {
        MapView {
            base,
            loader: None,
            roads: None,
            load_failed: false,
            car: None,
            origin: None,
            view: None,
            lang: None,
            open: false,
            drag_from: None,
            mouse_before: None,
        }
    }

    fn ready(&mut self) {
        // Input, polling and drawing must run while the tree is paused.
        self.base_mut().set_process_mode(ProcessMode::ALWAYS);
        self.base_mut()
            .set_anchors_and_offsets_preset(LayoutPreset::FULL_RECT);
        self.base_mut().set_mouse_filter(MouseFilter::STOP);
        self.base_mut().set_visible(false);
        self.base_mut().set_process(true);
        let locale = Os::singleton().get_locale().to_string();
        self.lang = Some(language(&locale));
    }

    fn process(&mut self, _delta: f64) {
        self.collect_roads();
        if self.open {
            self.base_mut().queue_redraw();
        }
    }

    fn unhandled_input(&mut self, event: Gd<InputEvent>) {
        if event.is_action_pressed("map_toggle") {
            self.set_open(!self.open);
        }
    }

    fn gui_input(&mut self, event: Gd<InputEvent>) {
        if !self.open {
            return;
        }
        let Some(size) = self.size() else {
            return;
        };
        if let Ok(button) = event.clone().try_cast::<InputEventMouseButton>() {
            let pos = button.get_position();
            match button.get_button_index() {
                MouseButton::WHEEL_UP if button.is_pressed() => self.zoom(size, pos, WHEEL_STEP),
                MouseButton::WHEEL_DOWN if button.is_pressed() => {
                    self.zoom(size, pos, 1.0 / WHEEL_STEP)
                }
                MouseButton::LEFT => {
                    self.drag_from = if button.is_pressed() { Some(pos) } else { None };
                }
                _ => return,
            }
            self.base_mut().accept_event();
            self.base_mut().queue_redraw();
        } else if let Ok(motion) = event.try_cast::<InputEventMouseMotion>() {
            if let (Some(from), Some(mut view)) = (self.drag_from, self.view) {
                let now = motion.get_position();
                view.pan_px((now.x - from.x) as f64, (now.y - from.y) as f64);
                self.view = Some(view);
                self.drag_from = Some(now);
                self.base_mut().queue_redraw();
                self.base_mut().accept_event();
            }
        }
    }

    fn draw(&mut self) {
        self.draw_map();
    }
}

impl MapView {
    /// Hands the map its data source: the map's index (roads are read in the background) and the
    /// car, with the node whose frame is the map's. Call once, after the car exists.
    pub(crate) fn start(&mut self, index: Arc<MapIndex>, car: Gd<Node3D>, origin: Gd<Node3D>) {
        self.loader = Some(spawn_load(index));
        self.car = Some(car);
        self.origin = Some(origin);
    }

    fn size(&self) -> Option<Vector2> {
        let size = self.base().get_size();
        (size.x > 0.0 && size.y > 0.0).then_some(size)
    }

    /// Takes the background load when it is ready; nothing is done while it runs.
    fn collect_roads(&mut self) {
        let Some(result) = self.loader.as_ref().and_then(MapLoad::poll) else {
            return;
        };
        self.loader = None;
        match result {
            Ok(roads) => {
                godot_print!(
                    "2D map: {} roads in {} tiles ready, {} tiles unreadable",
                    roads.road_count(),
                    roads.tile_count(),
                    roads.failed_tiles
                );
                self.roads = Some(roads);
            }
            Err(e) => {
                godot_error!("2D map: cannot read roads: {e}");
                self.load_failed = true;
            }
        }
    }

    fn zoom(&mut self, size: Vector2, pos: Vector2, factor: f64) {
        if let Some(mut view) = self.view {
            view.zoom_at(
                [size.x as f64, size.y as f64],
                [pos.x as f64, pos.y as f64],
                factor,
            );
            self.view = Some(view);
        }
    }

    /// Opens or closes the map. Opening pauses the game, frees the mouse and centers on the car.
    fn set_open(&mut self, open: bool) {
        if open == self.open {
            return;
        }
        self.open = open;
        self.drag_from = None;
        self.base_mut().set_visible(open);
        let mut tree = self.base().get_tree();
        tree.set_pause(open);
        let mut input = Input::singleton();
        if open {
            self.mouse_before = Some(input.get_mouse_mode());
            input.set_mouse_mode(MouseMode::VISIBLE);
            let center = self.car_map().map(|(p, _)| p).unwrap_or([0.0, 0.0]);
            self.view = Some(View::new(center, OPEN_M_PER_PX));
        } else if let Some(mode) = self.mouse_before.take() {
            input.set_mouse_mode(mode);
        }
        self.base_mut().queue_redraw();
    }

    /// Car position in map meters and its heading (unit vector, map frame), if the car exists.
    fn car_map(&self) -> Option<([f64; 2], [f64; 2])> {
        let car = self.car.as_ref()?;
        let origin = self.origin.as_ref()?;
        let local = origin.to_local(car.get_global_position());
        let pos = [local.x as f64, -local.z as f64];
        // The car drives along its -Z axis; in the map (x east, y north = -z) that is (-z.x, z.z),
        // where z is the car's own +Z axis (the basis column).
        let basis = origin.get_global_basis().inverse() * car.get_global_basis();
        let z = basis.col_c();
        let (hx, hy) = (-(z.x as f64), z.z as f64);
        let n = hx.hypot(hy).max(1e-9);
        Some((pos, [hx / n, hy / n]))
    }

    /// Draws the background, the roads, their names, the car and the hint.
    fn draw_map(&mut self) {
        let Some(size) = self.size() else {
            return;
        };
        self.base_mut()
            .draw_rect(Rect2::new(Vector2::ZERO, size), BACKGROUND);
        let lang = self.lang.unwrap_or(Lang::En);
        let Some(font) = ThemeDb::singleton().get_fallback_font() else {
            return;
        };
        let Some(view) = self.view else {
            return;
        };
        let Some(roads) = self.roads.as_ref() else {
            let key = if self.load_failed {
                "map.no_roads"
            } else {
                "map.loading"
            };
            self.draw_text_at(
                &font,
                size * 0.5,
                text(key, lang),
                TEXT,
                HorizontalAlignment::CENTER,
            );
            return;
        };
        let size_f = [size.x as f64, size.y as f64];
        let (min, max) = view.rect(size_f);
        let mut buckets: [PackedVector2Array; 4] = Default::default();
        let mut candidates: Vec<LabelCandidate> = Vec::new();
        let screen = |p: [f32; 2]| view.to_screen(size_f, [p[0] as f64, p[1] as f64]);
        roads.for_each_in_rect(min, max, |road| {
            if !shows_road(&road.highway, view.m_per_px) {
                return;
            }
            let pts: Vec<[f64; 2]> = road.points.iter().map(|&p| screen(p)).collect();
            let (lo, hi) = bounds(&pts);
            if hi[0] < 0.0 || hi[1] < 0.0 || lo[0] > size_f[0] || lo[1] > size_f[1] {
                return;
            }
            let pts = decimate(&pts, MIN_POINT_PX);
            let bucket = &mut buckets[(road_class(&road.highway) - 1) as usize];
            for seg in pts.windows(2) {
                bucket.push(Vector2::new(seg[0][0] as f32, seg[0][1] as f32));
                bucket.push(Vector2::new(seg[1][0] as f32, seg[1][1] as f32));
            }
            if !road.name.is_empty()
                && shows_label(&road.highway, view.m_per_px)
                && candidates.len() < MAX_LABEL_CANDIDATES
            {
                let (lo, hi) = bounds(&pts);
                let length_px = (hi[0] - lo[0]).hypot(hi[1] - lo[1]);
                if length_px >= MIN_LABEL_PX {
                    let raw: Vec<[f64; 2]> = road
                        .points
                        .iter()
                        .map(|p| [p[0] as f64, p[1] as f64])
                        .collect();
                    if let (Some((anchor, angle)), Some((anchor_map, _))) =
                        (label_anchor(&pts), label_anchor(&raw))
                    {
                        candidates.push(LabelCandidate {
                            name: road.name.clone(),
                            class: road_class(&road.highway),
                            length_px,
                            anchor,
                            angle,
                            anchor_map,
                        });
                    }
                }
            }
        });
        for (class, bucket) in buckets.iter().enumerate() {
            if !bucket.is_empty() {
                // One representative highway per class, for its width.
                let width = stroke_px(match class {
                    0 => "primary",
                    1 => "secondary",
                    2 => "residential",
                    _ => "service",
                });
                self.base_mut()
                    .draw_multiline_ex(bucket, ROAD)
                    .width(width)
                    .done();
            }
        }
        for i in select_labels(&candidates) {
            let c = &candidates[i];
            self.draw_label(&font, &c.name, c.anchor, c.angle);
        }
        self.draw_car(&view, size_f);
        self.draw_text_at(
            &font,
            Vector2::new(12.0, size.y - 14.0),
            text("map.hint", lang),
            TEXT_SHADOW,
            HorizontalAlignment::LEFT,
        );
        self.draw_text_at(
            &font,
            Vector2::new(size.x - 12.0, size.y - 14.0),
            text("map.attribution", lang),
            TEXT_SHADOW,
            HorizontalAlignment::RIGHT,
        );
    }

    /// The car as an arrow at its position, pointing along its heading (north is up on screen).
    fn draw_car(&mut self, view: &View, size: [f64; 2]) {
        let Some((pos, heading)) = self.car_map() else {
            return;
        };
        let c = view.to_screen(size, pos);
        // Screen direction: x as is, y flipped (north is up).
        let d = (heading[0], -heading[1]);
        let p = (-d.1, d.0);
        let at = |f: f64, s: f64| {
            Vector2::new(
                (c[0] + d.0 * f + p.0 * s) as f32,
                (c[1] + d.1 * f + p.1 * s) as f32,
            )
        };
        let points = PackedVector2Array::from([at(14.0, 0.0), at(-8.0, 8.0), at(-8.0, -8.0)]);
        self.base_mut().draw_colored_polygon(&points, CAR);
    }

    /// A road name, along the road, centered on its anchor, with a light outline.
    fn draw_label(&mut self, font: &Gd<Font>, name: &str, anchor: [f64; 2], angle: f64) {
        let pos = Vector2::new(anchor[0] as f32, anchor[1] as f32);
        let mut base = self.base_mut();
        base.draw_set_transform_ex(pos)
            .rotation(angle as f32)
            .done();
        let at = Vector2::new(0.0, -4.0);
        base.draw_string_outline_ex(font, at, name)
            .alignment(HorizontalAlignment::CENTER)
            .width(-1.0)
            .font_size(13)
            .size(3)
            .modulate(LABEL_OUTLINE)
            .done();
        base.draw_string_ex(font, at, name)
            .alignment(HorizontalAlignment::CENTER)
            .width(-1.0)
            .font_size(13)
            .modulate(LABEL)
            .done();
        base.draw_set_transform_ex(Vector2::ZERO)
            .rotation(0.0)
            .done();
    }

    /// Plain text with its baseline at `pos`.
    fn draw_text_at(
        &mut self,
        font: &Gd<Font>,
        pos: Vector2,
        s: &str,
        color: Color,
        align: HorizontalAlignment,
    ) {
        let base = self.base_mut();
        base.draw_string_ex(font, pos, s)
            .alignment(align)
            .width(-1.0)
            .font_size(14)
            .modulate(color)
            .done();
    }
}

/// Bounding box of screen points: `(min, max)`.
fn bounds(pts: &[[f64; 2]]) -> ([f64; 2], [f64; 2]) {
    let mut lo = [f64::MAX, f64::MAX];
    let mut hi = [f64::MIN, f64::MIN];
    for p in pts {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    (lo, hi)
}
