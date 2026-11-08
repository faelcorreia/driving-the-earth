//! GPS (item 93): a destination on the car road graph, the exact route to it from the car's road,
//! the direction to steer toward, the distance left, and a new route when the car leaves the old one.
//!
//! Threads: `load_map` opens the map and builds the graph and a grid over its nodes in a thread of
//! its own. A route (at `set_destination`, and again when the car leaves the route) is searched in a
//! thread of its own: one search at a time, at most one per second. The physics thread only projects
//! the car onto the route it already has (`update`), a short loop over the route's points.
//!
//! Coordinates are the local meters of the bake, `(x east, y north)`, as in `DriveRoute`. The car
//! script converts its own position (Godot `x`, `-z`) before calling `update`.
//!
//! Known limit, kept on purpose: the destination and the car are snapped to the nearest graph node,
//! not to the nearest edge (the graph's edges are not exposed by `pathfind.rs`). The route starts at
//! the car's own point, so the first metres follow the car, not the node.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Instant;

use godot::prelude::*;

use super::map::open_map;
use super::pathfind::CarGraph;

/// Distance from the car to the route, in m, past which a new route is searched.
const OFF_ROUTE_M: f64 = 25.0;
/// Shortest time between two route searches, in s.
const RECALC_MIN_S: f64 = 1.0;
/// Distance ahead on the route that the guide points at, in m.
const LOOKAHEAD_M: f64 = 30.0;
/// Distance from the destination at which the trip is over, in m.
const ARRIVE_M: f64 = 15.0;
/// Side of a grid cell over the graph nodes, in m.
const CELL_M: f64 = 50.0;
/// How far behind the last projection the car may be matched on the route, in m. Keeps a route
/// that passes the car twice (a loop) from being matched to its earlier pass.
const BACK_M: f64 = 25.0;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// The car road graph of a map, with a grid over its nodes to find the node nearest a point.
pub(crate) struct Net {
    graph: CarGraph,
    origin: [f64; 2],
    cells: HashMap<(i64, i64), Vec<u32>>,
    lo: (i64, i64),
    hi: (i64, i64),
}

fn cell_of(origin: [f64; 2], p: [f64; 2]) -> (i64, i64) {
    (
        ((p[0] - origin[0]) / CELL_M).floor() as i64,
        ((p[1] - origin[1]) / CELL_M).floor() as i64,
    )
}

impl Net {
    pub(crate) fn build(graph: CarGraph) -> Self {
        let n = graph.node_count();
        let mut origin = [f64::INFINITY; 2];
        for u in 0..n {
            let p = graph.position(u);
            origin[0] = origin[0].min(p[0]);
            origin[1] = origin[1].min(p[1]);
        }
        if n == 0 {
            origin = [0.0, 0.0];
        }
        let mut cells: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
        for u in 0..n {
            let c = cell_of(origin, graph.position(u));
            cells.entry(c).or_default().push(u as u32);
        }
        let (mut lo, mut hi) = ((i64::MAX, i64::MAX), (i64::MIN, i64::MIN));
        for &(x, y) in cells.keys() {
            lo = (lo.0.min(x), lo.1.min(y));
            hi = (hi.0.max(x), hi.1.max(y));
        }
        Net {
            graph,
            origin,
            cells,
            lo,
            hi,
        }
    }

    /// Node of the graph nearest to `p`; `None` when the graph has no node.
    pub(crate) fn nearest_node(&self, p: [f64; 2]) -> Option<usize> {
        if self.cells.is_empty() {
            return None;
        }
        let (cx, cy) = cell_of(self.origin, p);
        // Every cell of the grid lies within this many rings of (cx, cy).
        let far = (cx - self.lo.0)
            .abs()
            .max((cx - self.hi.0).abs())
            .max((cy - self.lo.1).abs())
            .max((cy - self.hi.1).abs());
        let mut best: Option<(f64, usize)> = None;
        for r in 0..=far {
            if r == 0 {
                self.scan_cell((cx, cy), p, &mut best);
            } else {
                for dx in -r..=r {
                    self.scan_cell((cx + dx, cy - r), p, &mut best);
                    self.scan_cell((cx + dx, cy + r), p, &mut best);
                }
                for dy in (-r + 1)..=(r - 1) {
                    self.scan_cell((cx - r, cy + dy), p, &mut best);
                    self.scan_cell((cx + r, cy + dy), p, &mut best);
                }
            }
            // Cells beyond ring r are at least r cells away from the cell of p.
            if let Some((bd, _)) = best {
                if bd <= r as f64 * CELL_M {
                    break;
                }
            }
        }
        best.map(|(_, u)| u)
    }

    /// Keeps in `best` the node of `cell` nearest to `p`, if it is nearer than the one kept.
    fn scan_cell(&self, cell: (i64, i64), p: [f64; 2], best: &mut Option<(f64, usize)>) {
        if let Some(list) = self.cells.get(&cell) {
            for &u in list {
                let d = dist(self.graph.position(u as usize), p);
                if best.map_or(true, |(bd, _)| d < bd) {
                    *best = Some((d, u as usize));
                }
            }
        }
    }
}

/// A route as the car drives it: the car's point first, then the graph nodes to the destination.
/// `cum[i]` is the distance along the route from its first point to `points[i]`.
#[derive(Clone)]
pub(crate) struct Track {
    points: Vec<[f64; 2]>,
    cum: Vec<f64>,
}

impl Track {
    pub(crate) fn new(points: Vec<[f64; 2]>) -> Self {
        let mut cum = Vec::with_capacity(points.len());
        let mut s = 0.0;
        for (i, p) in points.iter().enumerate() {
            if i > 0 {
                s += dist(points[i - 1], *p);
            }
            cum.push(s);
        }
        Track { points, cum }
    }

    pub(crate) fn length(&self) -> f64 {
        self.cum.last().copied().unwrap_or(0.0)
    }

    pub(crate) fn end(&self) -> [f64; 2] {
        self.points.last().copied().unwrap_or([0.0, 0.0])
    }

    /// Where `p` is on the route: its distance from the route, and the distance along the route of
    /// its closest point. Only the segments that end at `BACK_M` behind `from_s` or later are
    /// matched, so a loop is not taken for an earlier pass.
    pub(crate) fn project(&self, p: [f64; 2], from_s: f64) -> (f64, f64) {
        if self.points.len() == 1 {
            return (dist(p, self.points[0]), 0.0);
        }
        let mut best: Option<(f64, f64)> = None; // (distance, along)
        let consider = |i: usize, best: &mut Option<(f64, f64)>| {
            let a = self.points[i];
            let b = self.points[i + 1];
            let ab = [b[0] - a[0], b[1] - a[1]];
            let len2 = ab[0] * ab[0] + ab[1] * ab[1];
            let t = if len2 == 0.0 {
                0.0
            } else {
                (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0)
            };
            let q = [a[0] + ab[0] * t, a[1] + ab[1] * t];
            let d = dist(p, q);
            let s = self.cum[i] + t * len2.sqrt();
            if best.map_or(true, |(bd, _)| d < bd) {
                *best = Some((d, s));
            }
        };
        for i in 0..self.points.len() - 1 {
            if self.cum[i + 1] >= from_s - BACK_M {
                consider(i, &mut best);
            }
        }
        if best.is_none() {
            for i in 0..self.points.len() - 1 {
                consider(i, &mut best);
            }
        }
        best.unwrap_or((dist(p, self.points[0]), 0.0))
    }

    /// The point at distance `s` along the route (clamped to its ends).
    pub(crate) fn point_at(&self, s: f64) -> [f64; 2] {
        let total = self.length();
        let s = s.clamp(0.0, total);
        let i = self
            .cum
            .partition_point(|&c| c <= s)
            .clamp(1, self.points.len() - 1);
        let (s0, s1) = (self.cum[i - 1], self.cum[i]);
        let t = if s1 > s0 { (s - s0) / (s1 - s0) } else { 0.0 };
        let (a, b) = (self.points[i - 1], self.points[i]);
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
    }
}

/// Search of the route from `car` to `dest` on `net`: the car's point, then the graph nodes of the
/// shortest car route. `None` when a point has no node or the destination cannot be reached.
pub(crate) fn search_route(net: &Net, car: [f64; 2], dest: [f64; 2]) -> Option<Track> {
    let from = net.nearest_node(car)?;
    let to = net.nearest_node(dest)?;
    let route = net.graph.shortest_path(from, to)?;
    let mut points = Vec::with_capacity(route.points.len() + 1);
    points.push(car);
    points.extend(route.points);
    Some(Track::new(points))
}

/// A new route is searched when there is none or the car is off the route, and at most once a
/// second (`since_last_s` is the time since the last search began, `None` when there was none).
pub(crate) fn search_due(has_track: bool, off_route: bool, since_last_s: Option<f64>) -> bool {
    (!has_track || off_route) && since_last_s.map_or(true, |s| s >= RECALC_MIN_S)
}

/// The trip is over when the car is within `ARRIVE_M` of the destination.
pub(crate) fn is_arrived(car: [f64; 2], dest: [f64; 2]) -> bool {
    dist(car, dest) < ARRIVE_M
}

/// Unit direction, in the plane, from `car` to the route point `LOOKAHEAD_M` ahead of `s`; zero
/// when that point is the car's own point.
fn guide_vector(track: &Track, car: [f64; 2], s: f64) -> Vector2 {
    let g = track.point_at(s + LOOKAHEAD_M);
    let v = [g[0] - car[0], g[1] - car[1]];
    let n = v[0].hypot(v[1]);
    if n < 1e-9 {
        Vector2::ZERO
    } else {
        Vector2::new((v[0] / n) as f32, (v[1] / n) as f32)
    }
}

/// State of the map: nothing requested, being read (with the number of its request), ready, or failed.
enum MapState {
    Empty,
    Loading(u64),
    Ready(Arc<Net>),
    Failed(String),
}

/// The result of one route search, with the destination generation it was started for.
struct Search {
    generation: u64,
    track: Option<Track>,
}

#[derive(GodotClass)]
#[class(base = Node)]
pub struct Gps {
    base: Base<Node>,
    map: Arc<Mutex<MapState>>,
    map_requests: u64,
    /// Destination in the map's local meters, `None` when there is none.
    dest: Option<[f64; 2]>,
    /// Bumped by `set_destination` and `clear`; a search of an older generation is dropped.
    generation: u64,
    track: Option<Track>,
    /// Distance along the track of the last projection of the car.
    progress: f64,
    off_route: bool,
    guide: Vector2,
    remaining: f64,
    search_slot: Arc<Mutex<Option<Search>>>,
    searching: bool,
    last_search: Option<Instant>,
}

#[godot_api]
impl INode for Gps {
    fn init(base: Base<Node>) -> Self {
        Gps {
            base,
            map: Arc::new(Mutex::new(MapState::Empty)),
            map_requests: 0,
            dest: None,
            generation: 0,
            track: None,
            progress: 0.0,
            off_route: false,
            guide: Vector2::ZERO,
            remaining: 0.0,
            search_slot: Arc::new(Mutex::new(None)),
            searching: false,
            last_search: None,
        }
    }
}

#[godot_api]
impl Gps {
    /// Emitted once when the car comes within 15 m of the destination. The destination is then cleared.
    #[signal]
    fn arrived();

    /// Opens the baked map `map_id` under `cache_root` in a thread of its own; `is_loaded()` turns
    /// true when the graph is ready. The cache is only read.
    #[func]
    fn load_map(&mut self, cache_root: GString, map_id: GString) {
        self.map_requests += 1;
        let request = self.map_requests;
        *lock(&self.map) = MapState::Loading(request);
        let slot = Arc::clone(&self.map);
        let root = PathBuf::from(cache_root.to_string());
        let id = map_id.to_string();
        thread::spawn(move || {
            let state = match open_map(&root, &id).and_then(|index| CarGraph::load(&index)) {
                Ok(graph) => MapState::Ready(Arc::new(Net::build(graph))),
                Err(e) => MapState::Failed(format!("'{id}': {e}")),
            };
            if let MapState::Failed(msg) = &state {
                eprintln!("gps: cannot load map {msg}");
            }
            let mut current = lock(&slot);
            // Only the latest request may publish its state.
            if matches!(*current, MapState::Loading(r) if r == request) {
                *current = state;
            }
        });
    }

    /// True when the graph of the requested map is ready for routes.
    #[func]
    fn is_loaded(&self) -> bool {
        matches!(*lock(&self.map), MapState::Ready(_))
    }

    /// Sets the destination `(x, y)` in the map's local meters; the route is searched from the next
    /// `update`. Replaces any earlier destination.
    #[func]
    fn set_destination(&mut self, x: f64, y: f64) {
        self.dest = Some([x, y]);
        self.reset_route();
    }

    /// Clears the destination and the route.
    #[func]
    fn clear(&mut self) {
        self.dest = None;
        self.reset_route();
    }

    /// Feeds the car's position `(x, y)` in the map's local meters. Call it once per physics tick.
    /// Projects the car onto the route, emits `arrived`, and starts a route search when there is
    /// none or the car is off it (at most one a second, in a thread of its own).
    #[func]
    fn update(&mut self, car_x: f64, car_y: f64) {
        let car = [car_x, car_y];
        self.take_search();
        let Some(dest) = self.dest else {
            return;
        };
        let mut arrived = false;
        if let Some(track) = self.track.as_ref() {
            let (off, s) = track.project(car, self.progress);
            self.progress = s;
            arrived = is_arrived(car, track.end());
            if !arrived {
                self.off_route = off > OFF_ROUTE_M;
                self.remaining = (track.length() - s).max(0.0);
                self.guide = guide_vector(track, car, s);
            }
        }
        if arrived {
            self.clear();
            self.signals().arrived().emit();
            return;
        }
        let since = self.last_search.map(|t| t.elapsed().as_secs_f64());
        if !self.searching && search_due(self.track.is_some(), self.off_route, since) {
            self.start_search(car, dest);
        }
    }

    /// Unit direction in the plane from the car toward the route point 30 m ahead (never the straight
    /// line to the destination). Zero when there is no route.
    #[func]
    fn guide_direction(&self) -> Vector2 {
        self.guide
    }

    /// Distance left along the route, in m, from the car's last projection. Zero when there is no route.
    #[func]
    fn remaining_m(&self) -> f64 {
        self.remaining
    }

    /// True while there is a destination and a route to it.
    #[func]
    fn has_route(&self) -> bool {
        self.dest.is_some() && self.track.is_some()
    }
}

impl Gps {
    /// Drops the route and the guide, and starts a new destination generation.
    fn reset_route(&mut self) {
        self.generation += 1;
        self.track = None;
        self.progress = 0.0;
        self.off_route = false;
        self.guide = Vector2::ZERO;
        self.remaining = 0.0;
    }

    /// Takes the result of the search in progress, if it has finished. A result of an older
    /// destination is dropped.
    fn take_search(&mut self) {
        let finished = lock(&self.search_slot).take();
        if let Some(search) = finished {
            self.searching = false;
            if search.generation == self.generation && self.dest.is_some() {
                self.track = search.track;
                self.progress = 0.0;
                self.off_route = false;
            }
        }
    }

    fn start_search(&mut self, car: [f64; 2], dest: [f64; 2]) {
        let net = match &*lock(&self.map) {
            MapState::Ready(net) => Arc::clone(net),
            _ => return,
        };
        self.searching = true;
        self.last_search = Some(Instant::now());
        let slot = Arc::clone(&self.search_slot);
        let generation = self.generation;
        thread::spawn(move || {
            let track = search_route(&net, car, dest);
            *lock(&slot) = Some(Search { generation, track });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tileformat::Polyline;

    fn line(highway: &str, points: &[[f64; 2]]) -> Polyline {
        Polyline {
            highway: highway.into(),
            name: String::new(),
            points: points.to_vec(),
            z: vec![],
            zl: vec![],
            zr: vec![],
            lit: None,
            oneway: None,
            lanes: None,
            maxspeed: None,
            roundabout: false,
        }
    }

    /// An L of two-way roads: east along y = 0 from (0, 0) to (300, 0), then north to (300, 300).
    fn l_net() -> Net {
        let east: Vec<[f64; 2]> = (0..=6).map(|i| [i as f64 * 50.0, 0.0]).collect();
        let north: Vec<[f64; 2]> = (0..=6).map(|i| [300.0, i as f64 * 50.0]).collect();
        Net::build(CarGraph::from_polylines(vec![
            line("residential", &east),
            line("residential", &north),
        ]))
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    /// The guide follows the road's bend: at the start it points east along the road, not at the
    /// destination in a straight line; after the bend it points north-east toward the next road point.
    #[test]
    fn guide_follows_the_curve_not_the_chord() {
        let net = l_net();
        let track = search_route(&net, [0.0, 0.0], [300.0, 300.0]).expect("route");
        assert_eq!(track.end(), [300.0, 300.0]);
        assert!(close(track.length(), 600.0), "length {}", track.length());

        let (off, s) = track.project([0.0, 0.0], 0.0);
        assert!(off < 1e-9 && s < 1e-9);
        let g = guide_vector(&track, [0.0, 0.0], s);
        assert!(
            close(g.x as f64, 1.0) && close(g.y as f64, 0.0),
            "start guide {g:?}"
        );

        let (off, s) = track.project([290.0, 0.0], 0.0);
        assert!(off < 1e-9 && close(s, 290.0), "off {off} s {s}");
        let g = guide_vector(&track, [290.0, 0.0], s);
        let n = (10.0f64 * 10.0 + 20.0 * 20.0).sqrt();
        assert!(
            close(g.x as f64, 10.0 / n) && close(g.y as f64, 20.0 / n),
            "bend guide {g:?}"
        );
    }

    /// Off the road the car is far from the route, and that asks for a new one; on the road it
    /// does not. A new one is never asked for within a second of the last.
    #[test]
    fn recalculates_when_off_route_at_most_once_a_second() {
        let net = l_net();
        let track = search_route(&net, [0.0, 0.0], [300.0, 300.0]).unwrap();

        let (off, _) = track.project([150.0, 40.0], 0.0);
        assert!(off > OFF_ROUTE_M, "off {off}");
        assert!(search_due(true, off > OFF_ROUTE_M, Some(1.0)));
        assert!(!search_due(true, off > OFF_ROUTE_M, Some(0.5)));

        let (off, _) = track.project([150.0, 10.0], 0.0);
        assert!(off <= OFF_ROUTE_M, "off {off}");
        assert!(!search_due(true, false, Some(5.0)));

        assert!(
            search_due(false, false, None),
            "no route yet: search at once"
        );
        assert!(
            !search_due(false, false, Some(0.2)),
            "no route, but a search just ran"
        );
    }

    /// A new search from a point off the road starts at that point and still ends at the destination.
    #[test]
    fn search_from_off_road_point_reaches_destination() {
        let net = l_net();
        let track = search_route(&net, [150.0, 40.0], [300.0, 300.0]).expect("route");
        assert_eq!(track.points[0], [150.0, 40.0]);
        assert_eq!(track.end(), [300.0, 300.0]);
    }

    /// Arrival is inside 15 m of the destination, not at 15 m or beyond.
    #[test]
    fn arrival_is_within_15_m() {
        assert!(is_arrived([0.0, 0.0], [14.9, 0.0]));
        assert!(!is_arrived([0.0, 0.0], [15.1, 0.0]));
    }

    /// The grid finds the same nearest node as a scan of all nodes.
    #[test]
    fn nearest_node_matches_a_scan() {
        let mut seed: u64 = 93;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut lines = Vec::new();
        for _ in 0..40 {
            let (x, y) = (next() * 2000.0, next() * 2000.0);
            let pts: Vec<[f64; 2]> = (0..6)
                .map(|i| [x + i as f64 * 37.0, y + next() * 90.0])
                .collect();
            lines.push(line("residential", &pts));
        }
        let net = Net::build(CarGraph::from_polylines(lines));
        for _ in 0..200 {
            let p = [next() * 2600.0 - 300.0, next() * 2600.0 - 300.0];
            let got = net.nearest_node(p).unwrap();
            let best = (0..net.graph.node_count())
                .map(|u| dist(net.graph.position(u), p))
                .fold(f64::INFINITY, f64::min);
            assert!(close(dist(net.graph.position(got), p), best), "p {p:?}");
        }
    }
}
