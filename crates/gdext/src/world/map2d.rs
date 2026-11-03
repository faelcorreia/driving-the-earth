//! 2D map data and view math (F15a): the roads of the baked map grouped by tile, the projection
//! from map meters to screen pixels, zoom limits, and which road classes and labels show at each
//! zoom. Pure: no Godot types. Map coordinates are the bake's local plane (x east, y north, in
//! meters); the screen is north up. The roads are read in the background from the map on disk.
use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use tileformat::Polyline;

use super::map::MapIndex;

/// Closest zoom, in meters per screen pixel.
pub const MIN_M_PER_PX: f64 = 0.25;
/// Farthest zoom: the whole 10 km cache fits on a screen at about 10 m per pixel.
pub const MAX_M_PER_PX: f64 = 40.0;

/// Road class by OSM `highway` value: 1 the main roads, 4 everything else (service, paths).
pub fn road_class(highway: &str) -> u8 {
    match highway {
        "motorway" | "trunk" | "primary" => 1,
        "secondary" | "tertiary" => 2,
        "residential" | "unclassified" | "living_street" => 3,
        _ => 4,
    }
}

/// Coarsest zoom (m per pixel) at which a class is drawn at all.
pub fn class_limit_m_per_px(class: u8) -> f64 {
    match class {
        1 => 40.0,
        2 => 16.0,
        3 => 4.0,
        _ => 1.5,
    }
}

/// Coarsest zoom at which a class gets its name written on it.
pub fn label_limit_m_per_px(class: u8) -> f64 {
    match class {
        1 => 16.0,
        2 => 6.0,
        3 => 2.0,
        _ => 0.0,
    }
}

/// Whether a road of this `highway` value is drawn at this zoom.
pub fn shows_road(highway: &str, m_per_px: f64) -> bool {
    m_per_px <= class_limit_m_per_px(road_class(highway))
}

/// Whether a road of this `highway` value gets its name written at this zoom.
pub fn shows_label(highway: &str, m_per_px: f64) -> bool {
    m_per_px <= label_limit_m_per_px(road_class(highway))
}

/// Width of a road on screen, in pixels: the main roads are the thickest.
pub fn stroke_px(highway: &str) -> f32 {
    match road_class(highway) {
        1 => 5.0,
        2 => 4.0,
        3 => 3.0,
        _ => 2.0,
    }
}

/// Keeps a zoom (meters per pixel) inside the limits.
pub fn clamp_m_per_px(m_per_px: f64) -> f64 {
    m_per_px.clamp(MIN_M_PER_PX, MAX_M_PER_PX)
}

/// One road piece of the map: its OSM class and name, and its points in map meters.
#[derive(Debug, Clone, PartialEq)]
pub struct Road2d {
    pub highway: String,
    pub name: String,
    pub points: Vec<[f32; 2]>,
}

/// The roads of a whole baked map, grouped by tile `(x, y)` (tile `x` covers `[x * tile_m, (x + 1) * tile_m)`).
#[derive(Debug, Default)]
pub struct MapRoads {
    tile_m: f64,
    tiles: HashMap<(i32, i32), Vec<Road2d>>,
    /// Tiles that could not be read; their roads are missing from the map.
    pub failed_tiles: usize,
}

impl MapRoads {
    pub fn new(tile_m: f64) -> Self {
        MapRoads {
            tile_m,
            tiles: HashMap::new(),
            failed_tiles: 0,
        }
    }

    /// Adds the roads of tile `(x, y)`. Pieces with fewer than two points are dropped.
    pub fn add_tile(&mut self, x: i32, y: i32, roads: &[Polyline]) {
        let pieces: Vec<Road2d> = roads
            .iter()
            .filter(|r| r.points.len() >= 2)
            .map(|r| Road2d {
                highway: r.highway.clone(),
                name: r.name.clone(),
                points: r
                    .points
                    .iter()
                    .map(|p| [p[0] as f32, p[1] as f32])
                    .collect(),
            })
            .collect();
        if !pieces.is_empty() {
            self.tiles.insert((x, y), pieces);
        }
    }

    pub fn road_count(&self) -> usize {
        self.tiles.values().map(Vec::len).sum()
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Calls `f` for every road of the tiles that touch the rectangle `[min, max]` (map meters).
    /// Roads of those tiles may still lie outside the rectangle: the caller checks the points.
    pub fn for_each_in_rect(&self, min: [f64; 2], max: [f64; 2], mut f: impl FnMut(&Road2d)) {
        let t = self.tile_m;
        let (x0, x1) = ((min[0] / t).floor() as i64, (max[0] / t).floor() as i64);
        let (y0, y1) = ((min[1] / t).floor() as i64, (max[1] / t).floor() as i64);
        let cells = (x1 - x0 + 1).max(0) * (y1 - y0 + 1).max(0);
        if cells as usize > self.tiles.len() {
            // Wider than the map: walk the tiles that exist instead of every cell.
            for (&(x, y), roads) in &self.tiles {
                if (x as i64) >= x0 && (x as i64) <= x1 && (y as i64) >= y0 && (y as i64) <= y1 {
                    roads.iter().for_each(&mut f);
                }
            }
            return;
        }
        for y in y0..=y1 {
            for x in x0..=x1 {
                if let Some(roads) = self.tiles.get(&(x as i32, y as i32)) {
                    roads.iter().for_each(&mut f);
                }
            }
        }
    }
}

/// A road load running in a background thread. Dropping it stops the thread (after the tile it
/// is reading) and waits for it, so the thread never outlives the game's library.
pub struct MapLoad {
    result: Receiver<io::Result<MapRoads>>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MapLoad {
    /// The loaded roads, once the thread is done. `None` while it runs.
    pub fn poll(&self) -> Option<io::Result<MapRoads>> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(io::Error::other("map loader gone"))),
        }
    }
}

impl Drop for MapLoad {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Opens the map's roads in a background thread. Reads `roads.bin` (one file) when the bake wrote
/// it, else every tile's road pieces. The game keeps running meanwhile.
pub fn spawn_load(index: Arc<MapIndex>) -> MapLoad {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = cancel.clone();
    let thread = thread::spawn(move || {
        let _ = tx.send(load_roads(&index, &stop));
    });
    MapLoad {
        result: rx,
        cancel,
        thread: Some(thread),
    }
}

/// The map's roads, in the thread of `spawn_load`. `stop` is checked between tiles.
fn load_roads(index: &MapIndex, stop: &AtomicBool) -> io::Result<MapRoads> {
    let mut roads = MapRoads::new(index.meta.tile_size_m);
    let file = index.roads_file();
    if file.is_file() {
        for (x, y, pieces) in tileformat::roads::read_roads_file(&file)? {
            roads.add_tile(x, y, &pieces);
        }
        return Ok(roads);
    }
    let mut any_ok = false;
    for &(x, y) in index.coords() {
        if stop.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "map load stopped",
            ));
        }
        match index.read_roads_at(x, y) {
            Ok(pieces) => {
                any_ok = true;
                roads.add_tile(x, y, &pieces);
            }
            Err(_) => roads.failed_tiles += 1,
        }
    }
    if any_ok || index.coords().is_empty() {
        Ok(roads)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "no tile of the map could be read",
        ))
    }
}

/// Estimated size of a name drawn on screen: a character is `LABEL_CHAR_PX` wide, the line `LABEL_HEIGHT_PX` high.
pub const LABEL_CHAR_PX: f64 = 7.0;
pub const LABEL_HEIGHT_PX: f64 = 16.0;
/// Two pieces with the same name whose anchors are closer than this (map meters) are one stretch of
/// street, so only one of them is named. This is how a way cut at a tile border gets one name.
pub const SAME_NAME_M: f64 = 300.0;

/// A road name that may be written at this zoom.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelCandidate {
    pub name: String,
    /// `road_class` of the road: 1 is the most important.
    pub class: u8,
    /// On-screen length of the piece, in pixels (the longer, the better place for its name).
    pub length_px: f64,
    /// Center of the name on screen, in pixels, and its angle (radians, screen axes).
    pub anchor: [f64; 2],
    pub angle: f64,
    /// Same anchor in map meters, to recognize one stretch of street across tiles.
    pub anchor_map: [f64; 2],
}

/// Axis-aligned box `[min_x, min_y, max_x, max_y]` around the rotated name.
pub fn label_box(c: &LabelCandidate) -> [f64; 4] {
    let half_w = c.name.chars().count() as f64 * LABEL_CHAR_PX / 2.0;
    let half_h = LABEL_HEIGHT_PX / 2.0;
    let (sin, cos) = c.angle.sin_cos();
    let ex = cos.abs() * half_w + sin.abs() * half_h;
    let ey = sin.abs() * half_w + cos.abs() * half_h;
    [
        c.anchor[0] - ex,
        c.anchor[1] - ey,
        c.anchor[0] + ex,
        c.anchor[1] + ey,
    ]
}

fn boxes_overlap(a: &[f64; 4], b: &[f64; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// Chooses which names to write: the most important roads first (then the longer pieces), and a name
/// is dropped when it would overlap a name already chosen or repeat one chosen for the same stretch.
/// Returns the indices of the chosen candidates. Dropped names come back on their own when zooming in,
/// because the choice is made again for every view.
pub fn select_labels(cands: &[LabelCandidate]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..cands.len()).collect();
    order.sort_by(|&a, &b| {
        cands[a]
            .class
            .cmp(&cands[b].class)
            .then(cands[b].length_px.total_cmp(&cands[a].length_px))
            .then(a.cmp(&b))
    });
    let mut chosen: Vec<usize> = Vec::new();
    let mut boxes: Vec<[f64; 4]> = Vec::new();
    for i in order {
        let c = &cands[i];
        let same_stretch = chosen.iter().any(|&j| {
            let o = &cands[j];
            o.name == c.name
                && (o.anchor_map[0] - c.anchor_map[0]).hypot(o.anchor_map[1] - c.anchor_map[1])
                    < SAME_NAME_M
        });
        if same_stretch {
            continue;
        }
        let b = label_box(c);
        if boxes.iter().any(|o| boxes_overlap(o, &b)) {
            continue;
        }
        chosen.push(i);
        boxes.push(b);
    }
    chosen
}

/// The view: which map point is at the center of the screen, and the zoom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub center: [f64; 2],
    pub m_per_px: f64,
}

impl View {
    pub fn new(center: [f64; 2], m_per_px: f64) -> Self {
        View {
            center,
            m_per_px: clamp_m_per_px(m_per_px),
        }
    }

    /// Screen pixel of map point `p`, for a screen of `size` pixels (north up, y down).
    pub fn to_screen(&self, size: [f64; 2], p: [f64; 2]) -> [f64; 2] {
        [
            size[0] / 2.0 + (p[0] - self.center[0]) / self.m_per_px,
            size[1] / 2.0 - (p[1] - self.center[1]) / self.m_per_px,
        ]
    }

    /// Map point under screen pixel `s`.
    pub fn to_map(&self, size: [f64; 2], s: [f64; 2]) -> [f64; 2] {
        [
            self.center[0] + (s[0] - size[0] / 2.0) * self.m_per_px,
            self.center[1] + (size[1] / 2.0 - s[1]) * self.m_per_px,
        ]
    }

    /// The map rectangle on screen: `(min, max)` corners in map meters.
    pub fn rect(&self, size: [f64; 2]) -> ([f64; 2], [f64; 2]) {
        (
            self.to_map(size, [0.0, size[1]]),
            self.to_map(size, [size[0], 0.0]),
        )
    }

    /// Zooms by `factor` (below 1 zooms in) keeping the map point under screen pixel `s` in place.
    pub fn zoom_at(&mut self, size: [f64; 2], s: [f64; 2], factor: f64) {
        let p = self.to_map(size, s);
        self.m_per_px = clamp_m_per_px(self.m_per_px * factor);
        self.center = [
            p[0] - (s[0] - size[0] / 2.0) * self.m_per_px,
            p[1] - (size[1] / 2.0 - s[1]) * self.m_per_px,
        ];
    }

    /// Moves the map with the mouse: the map follows the pointer by `(dx, dy)` pixels.
    pub fn pan_px(&mut self, dx: f64, dy: f64) {
        self.center[0] -= dx * self.m_per_px;
        self.center[1] += dy * self.m_per_px;
    }
}

/// Drops the points of a polyline that fall within `min_px` of the last kept one. The last point
/// is always kept, so the line keeps its end.
pub fn decimate(points: &[[f64; 2]], min_px: f64) -> Vec<[f64; 2]> {
    let mut out: Vec<[f64; 2]> = Vec::with_capacity(points.len());
    for (i, p) in points.iter().enumerate() {
        match out.last() {
            None => out.push(*p),
            Some(last) => {
                let d = (p[0] - last[0]).hypot(p[1] - last[1]);
                if d >= min_px || i + 1 == points.len() {
                    out.push(*p);
                }
            }
        }
    }
    out
}

/// Where to write a road's name: the midpoint of its longest segment, and the angle of that segment
/// in radians, turned so that the text is never upside down (in `(-PI/2, PI/2]`).
pub fn label_anchor(points: &[[f64; 2]]) -> Option<([f64; 2], f64)> {
    let (a, b) = points.windows(2).map(|w| (w[0], w[1])).max_by(|p, q| {
        let len = |(a, b): ([f64; 2], [f64; 2])| (b[0] - a[0]).hypot(b[1] - a[1]);
        len(*p).total_cmp(&len(*q))
    })?;
    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let mut angle = (b[1] - a[1]).atan2(b[0] - a[0]);
    if angle > std::f64::consts::FRAC_PI_2 {
        angle -= std::f64::consts::PI;
    } else if angle <= -std::f64::consts::FRAC_PI_2 {
        angle += std::f64::consts::PI;
    }
    Some((mid, angle))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: [f64; 2] = [800.0, 600.0];

    #[test]
    fn projection_is_north_up_and_round_trips() {
        let view = View::new([1000.0, 2000.0], 2.0);
        // The center is at the middle of the screen; north (larger y) is up, east is right.
        assert_eq!(view.to_screen(SIZE, [1000.0, 2000.0]), [400.0, 300.0]);
        let north = view.to_screen(SIZE, [1000.0, 2010.0]);
        assert!(north[1] < 300.0 && (north[1] - 295.0).abs() < 1e-9);
        let east = view.to_screen(SIZE, [1010.0, 2000.0]);
        assert!(east[0] > 400.0 && (east[0] - 405.0).abs() < 1e-9);
        let p = view.to_map(SIZE, view.to_screen(SIZE, [1234.5, 1876.25]));
        assert!((p[0] - 1234.5).abs() < 1e-9 && (p[1] - 1876.25).abs() < 1e-9);
    }

    #[test]
    fn rect_is_the_visible_part_of_the_map() {
        let view = View::new([0.0, 0.0], 1.0);
        let (min, max) = view.rect(SIZE);
        assert_eq!(min, [-400.0, -300.0]);
        assert_eq!(max, [400.0, 300.0]);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        let mut view = View::new([500.0, 500.0], 4.0);
        let cursor = [150.0, 520.0];
        let before = view.to_map(SIZE, cursor);
        view.zoom_at(SIZE, cursor, 0.5);
        assert_eq!(view.m_per_px, 2.0);
        let after = view.to_map(SIZE, cursor);
        assert!((before[0] - after[0]).abs() < 1e-9 && (before[1] - after[1]).abs() < 1e-9);
    }

    #[test]
    fn zoom_stops_at_the_limits() {
        let mut view = View::new([0.0, 0.0], 1.0);
        for _ in 0..50 {
            view.zoom_at(SIZE, [400.0, 300.0], 0.5);
        }
        assert_eq!(view.m_per_px, MIN_M_PER_PX);
        for _ in 0..50 {
            view.zoom_at(SIZE, [400.0, 300.0], 2.0);
        }
        assert_eq!(view.m_per_px, MAX_M_PER_PX);
    }

    #[test]
    fn pan_moves_the_map_with_the_pointer() {
        let mut view = View::new([0.0, 0.0], 2.0);
        let before = view.to_screen(SIZE, [100.0, 100.0]);
        view.pan_px(10.0, -5.0);
        let after = view.to_screen(SIZE, [100.0, 100.0]);
        assert!((after[0] - before[0] - 10.0).abs() < 1e-9);
        assert!((after[1] - before[1] + 5.0).abs() < 1e-9);
    }

    #[test]
    fn main_roads_show_at_every_zoom_and_small_ones_only_close_up() {
        assert!(shows_road("primary", MAX_M_PER_PX));
        assert!(!shows_road("secondary", 20.0) && shows_road("secondary", 10.0));
        assert!(!shows_road("residential", 10.0) && shows_road("residential", 3.0));
        assert!(!shows_road("service", 3.0) && shows_road("service", 1.0));
        assert!(!shows_label("tertiary", 10.0) && shows_label("tertiary", 5.0));
        assert!(shows_label("trunk", 16.0) && !shows_label("service", 1.0));
        // Thicker for the main roads.
        assert!(
            stroke_px("primary") > stroke_px("residential")
                && stroke_px("residential") > stroke_px("footway")
        );
    }

    #[test]
    fn decimate_drops_points_closer_than_a_pixel() {
        let pts = [[0.0, 0.0], [0.2, 0.0], [0.4, 0.0], [3.0, 0.0], [3.1, 0.0]];
        // 3.1 is the last point: kept even though it is close to 3.0.
        assert_eq!(
            decimate(&pts, 1.0),
            vec![[0.0, 0.0], [3.0, 0.0], [3.1, 0.0]]
        );
        assert_eq!(decimate(&pts[..1], 1.0), vec![[0.0, 0.0]]);
    }

    #[test]
    fn label_sits_on_the_longest_segment_and_reads_left_to_right() {
        let (mid, angle) = label_anchor(&[[0.0, 0.0], [10.0, 0.0], [10.0, 50.0]]).unwrap();
        assert_eq!(mid, [10.0, 25.0]);
        assert!((angle - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        // A segment running west to east back-to-front reads the other way round.
        let (_, angle) = label_anchor(&[[10.0, 0.0], [0.0, 0.0]]).unwrap();
        assert!(angle.abs() < 1e-9);
        assert!(label_anchor(&[[0.0, 0.0]]).is_none());
    }

    fn road(highway: &str, name: &str, pts: &[[f64; 2]]) -> Polyline {
        Polyline {
            highway: highway.into(),
            name: name.into(),
            points: pts.to_vec(),
            z: vec![],
            zl: vec![],
            zr: vec![],
        }
    }

    #[test]
    fn roads_are_found_by_the_tiles_the_rectangle_touches() {
        let mut map = MapRoads::new(250.0);
        map.add_tile(
            0,
            0,
            &[road("primary", "A", &[[10.0, 10.0], [200.0, 10.0]])],
        );
        map.add_tile(
            1,
            0,
            &[road("residential", "B", &[[300.0, 10.0], [400.0, 10.0]])],
        );
        map.add_tile(
            5,
            5,
            &[road(
                "primary",
                "far",
                &[[1300.0, 1300.0], [1400.0, 1300.0]],
            )],
        );
        map.add_tile(9, 9, &[road("primary", "dropped", &[[1.0, 1.0]])]);
        assert_eq!(map.road_count(), 3, "a piece with one point is dropped");
        let mut names = Vec::new();
        map.for_each_in_rect([0.0, 0.0], [260.0, 100.0], |r| names.push(r.name.clone()));
        names.sort();
        assert_eq!(names, vec!["A".to_string(), "B".to_string()]);
        // A rectangle wider than the map visits every tile once.
        let mut count = 0;
        map.for_each_in_rect([-1e6, -1e6], [1e6, 1e6], |_| count += 1);
        assert_eq!(count, 3);
        // An empty rectangle visits nothing.
        let mut none = 0;
        map.for_each_in_rect([2000.0, 2000.0], [1000.0, 1000.0], |_| none += 1);
        assert_eq!(none, 0);
    }

    fn cand(name: &str, class: u8, len: f64, anchor: [f64; 2], map: [f64; 2]) -> LabelCandidate {
        LabelCandidate {
            name: name.into(),
            class,
            length_px: len,
            anchor,
            angle: 0.0,
            anchor_map: map,
        }
    }

    #[test]
    fn label_box_grows_with_text_and_turns_with_the_road() {
        let flat = cand("Rua A", 3, 100.0, [100.0, 100.0], [0.0, 0.0]);
        let b = label_box(&flat);
        assert!((b[2] - b[0] - 5.0 * LABEL_CHAR_PX).abs() < 1e-9);
        assert!((b[3] - b[1] - LABEL_HEIGHT_PX).abs() < 1e-9);
        let mut upright = flat.clone();
        upright.angle = std::f64::consts::FRAC_PI_2;
        let u = label_box(&upright);
        assert!(
            (u[2] - u[0] - LABEL_HEIGHT_PX).abs() < 1e-9
                && (u[3] - u[1] - 5.0 * LABEL_CHAR_PX).abs() < 1e-9
        );
    }

    #[test]
    fn overlapping_names_keep_the_more_important_road() {
        // A short main road and a long residential one with names on the same spot.
        let main = cand("Avenida Afonso Pena", 1, 80.0, [400.0, 300.0], [0.0, 0.0]);
        let small = cand("Rua Pequena", 3, 300.0, [405.0, 302.0], [900.0, 900.0]);
        let list = [small, main];
        let kept: Vec<&str> = select_labels(&list)
            .iter()
            .map(|&i| list[i].name.as_str())
            .collect();
        assert_eq!(kept, vec!["Avenida Afonso Pena"]);
        // Same class: the longer piece wins.
        let long = cand("Rua Longa", 3, 300.0, [400.0, 300.0], [0.0, 0.0]);
        let short = cand("Rua Curta", 3, 60.0, [402.0, 300.0], [900.0, 900.0]);
        let list = [short, long];
        let kept: Vec<&str> = select_labels(&list)
            .iter()
            .map(|&i| list[i].name.as_str())
            .collect();
        assert_eq!(kept, vec!["Rua Longa"]);
    }

    #[test]
    fn apart_names_are_all_kept() {
        let a = cand("Rua A", 3, 200.0, [100.0, 100.0], [0.0, 0.0]);
        let b = cand("Rua B", 3, 200.0, [400.0, 100.0], [800.0, 0.0]);
        assert_eq!(select_labels(&[a, b]).len(), 2);
    }

    #[test]
    fn same_name_on_one_stretch_is_written_once_and_distant_repeats_stay() {
        // Two pieces of one street cut at a tile border: 120 m apart on the map.
        let first = cand("Rua São Mateus", 3, 200.0, [100.0, 100.0], [0.0, 0.0]);
        let second = cand("Rua São Mateus", 3, 150.0, [500.0, 100.0], [120.0, 0.0]);
        assert_eq!(select_labels(&[first.clone(), second]).len(), 1);
        // Another stretch of the same name, 1000 m away, keeps its own name.
        let far = cand("Rua São Mateus", 3, 150.0, [900.0, 400.0], [1000.0, 0.0]);
        assert_eq!(select_labels(&[first, far]).len(), 2);
    }

    #[test]
    fn names_hidden_by_zoom_out_come_back_when_zooming_in() {
        // Zoomed out: the anchors of two names are 30 px apart, less than their 35 px width, so they collide.
        let out = |x: f64| cand("Rua X", 3, 200.0, [x, 100.0], [x * 10.0, 0.0]);
        let zoomed_out = [
            out(100.0),
            cand("Rua Y", 3, 200.0, [130.0, 100.0], [1400.0, 0.0]),
        ];
        assert_eq!(select_labels(&zoomed_out).len(), 1);
        // Zoomed in: the same two names are 160 px apart and both are written.
        let zoomed_in = [
            cand("Rua X", 3, 200.0, [100.0, 100.0], [0.0, 0.0]),
            cand("Rua Y", 3, 200.0, [260.0, 100.0], [1400.0, 0.0]),
        ];
        assert_eq!(select_labels(&zoomed_in).len(), 2);
    }

    /// Real cache: how long the background read of every road takes. Run with
    /// `cargo test -p driving_the_earth_ext -- --ignored reads_the_roads_of_the_10km_cache --nocapture`.
    #[test]
    #[ignore]
    fn reads_the_roads_of_the_10km_cache() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache");
        let index = Arc::new(crate::world::map::open_map(&root, "casa-10km").unwrap());
        let tiles = index.coords().len();
        let started = std::time::Instant::now();
        let load = spawn_load(index);
        let roads = loop {
            if let Some(result) = load.poll() {
                break result.unwrap();
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        println!(
            "2D map: {} tiles, {} road pieces, {} unreadable, read in {:.0} ms",
            tiles,
            roads.road_count(),
            roads.failed_tiles,
            started.elapsed().as_secs_f64() * 1000.0
        );
        assert_eq!(roads.failed_tiles, 0);
        assert!(roads.road_count() > 0);
    }
}
