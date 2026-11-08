//! Fixed driving route for the drive bot (item 4). From a start point and heading on the map,
//! walks the road graph and keeps the straightest way at every junction, so the same start always
//! gives the same route. Local meters: `x` east, `y` north (see `world::map`).
//!
//! The graph is built from the road pieces of `roads.bin`: each consecutive pair of points is an
//! edge, both ways, and points closer than `NODE_GRID_M` are the same node.
use std::collections::HashMap;
use std::path::PathBuf;

use godot::prelude::*;
use tileformat::roads::read_roads_file;

use super::map::open_map;
use super::pathfind::{CarGraph, CarRoute};

/// Points within this distance, in meters, are the same graph node.
const NODE_GRID_M: f64 = 0.01;
/// OSM `highway` values a car can drive on. Footways and paths are left out: a footway piece can
/// end where the road beside it goes on, and the route would stop there.
pub(super) const DRIVABLE_HIGHWAYS: &[&str] = &[
    "motorway",
    "motorway_link",
    "trunk",
    "trunk_link",
    "primary",
    "primary_link",
    "secondary",
    "secondary_link",
    "tertiary",
    "tertiary_link",
    "unclassified",
    "residential",
    "living_street",
    "service",
    "road",
    "track",
];
/// Road endpoints this close, in meters, are joined even when they are not the same node.
const SNAP_M: f64 = 2.0;
/// Extra distance, in meters, around the start that is read from the roads file.
const READ_MARGIN_M: f64 = 100.0;
/// A junction turn sharper than this, in degrees, ends the route (the road turns back on itself).
const MAX_TURN_DEG: f64 = 135.0;
/// Speed used on the snap joins between road ends, in m/s (30 km/h).
pub(super) const SNAP_SPEED_MPS: f64 = 30.0 / 3.6;
/// Bend penalty of a turn of 180 degrees inside one way, in seconds. A turn of `BEND_MIN_DEG` or
/// less costs nothing; above it the penalty grows linearly up to this value.
const BEND_PENALTY_S: f64 = 4.0;
const BEND_MIN_DEG: f64 = 30.0;
/// Origins drawn for a random route (item 92) before giving up, and destinations tried per origin.
const RANDOM_ORIGIN_TRIES: usize = 200;
const RANDOM_DEST_TRIES: usize = 300;

/// Walks the road graph from `start` along `heading` (unit vector) for `length_m` meters and
/// returns the points of the route, the first one being `start` projected on the first road
/// segment. Ends early at a dead end.
pub fn fixed_route(
    roads: &[Vec<[f64; 2]>],
    start: [f64; 2],
    heading: [f64; 2],
    length_m: f64,
) -> Vec<[f64; 2]> {
    let pieces: Vec<Piece> = roads
        .iter()
        .map(|r| Piece {
            points: r,
            oneway: false,
            speed_mps: 1.0,
        })
        .collect();
    let graph = RoadGraph::build(&pieces);
    let Some((first_from, first_to, projected)) = graph.nearest_ahead(start, heading) else {
        return Vec::new();
    };

    let mut route = vec![projected];
    let mut total = dist(projected, graph.pos[first_to]);
    let mut prev = first_from;
    let mut cur = first_to;
    route.push(graph.pos[cur]);

    while total < length_m {
        let here = graph.pos[cur];
        let dir = unit(sub(here, graph.pos[prev]));
        let next = graph.adj[cur]
            .iter()
            .copied()
            .filter(|&n| n != prev)
            .map(|n| (turn_deg(dir, sub(graph.pos[n], here)), n))
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let Some((turn, n)) = next else { break };
        if turn > MAX_TURN_DEG {
            break;
        }
        total += dist(here, graph.pos[n]);
        route.push(graph.pos[n]);
        prev = cur;
        cur = n;
    }
    route
}

/// Random car route of `min_m`..`max_m` meters (item 92), by the car graph of item 91: an origin node
/// and a destination node drawn from `seed`. The length is the one of the shortest car route (its
/// `length_m`), not the straight line. The same seed on the same graph gives the same route.
pub(super) fn random_car_route(
    graph: &CarGraph,
    seed: u64,
    min_m: f64,
    max_m: f64,
) -> Option<CarRoute> {
    let n = graph.node_count();
    if n < 2 {
        return None;
    }
    let mut rng = SplitMix(seed);
    for _ in 0..RANDOM_ORIGIN_TRIES {
        let from = rng.below(n);
        let a = graph.position(from);
        // A car route is never shorter than the straight line: nodes farther than max_m cannot end it.
        let mut near: Vec<usize> = (0..n)
            .filter(|&v| {
                let d = dist(a, graph.position(v));
                d > 0.0 && d <= max_m && d >= min_m * 0.5
            })
            .collect();
        for i in (1..near.len()).rev() {
            near.swap(i, rng.below(i + 1));
        }
        for &to in near.iter().take(RANDOM_DEST_TRIES) {
            if let Some(route) = graph.shortest_path(from, to) {
                if route.length_m >= min_m && route.length_m <= max_m {
                    return Some(route);
                }
            }
        }
    }
    None
}

/// SplitMix64: a small generator for the random route, the same on every platform.
struct SplitMix(u64);

impl SplitMix {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (n > 0). The modulo bias is negligible for the node counts here.
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// One road piece as the graph reads it: its points in driving order, whether it is one-way
/// (traffic only from `points[0]` to the last point) and the speed on it, in m/s.
pub(super) struct Piece<'a> {
    pub points: &'a [[f64; 2]],
    pub oneway: bool,
    pub speed_mps: f64,
}

/// Road graph: nodes are the quantized points. `adj[u][i]` is the node reached from `u` by one edge,
/// with its length in meters (`len[u][i]`) and its travel time in seconds (`cost[u][i]`). Edges that
/// the pieces give twice keep the lower cost.
pub(super) struct RoadGraph {
    pub(super) pos: Vec<[f64; 2]>,
    pub(super) adj: Vec<Vec<usize>>,
    pub(super) len: Vec<Vec<f64>>,
    pub(super) cost: Vec<Vec<f64>>,
}

/// Growing graph plus the index of each directed edge `(a, b)` in `adj[a]`.
struct Builder {
    ids: HashMap<(i64, i64), usize>,
    graph: RoadGraph,
    slot: HashMap<(usize, usize), usize>,
}

impl Builder {
    fn node(&mut self, p: [f64; 2]) -> usize {
        let key = (
            (p[0] / NODE_GRID_M).round() as i64,
            (p[1] / NODE_GRID_M).round() as i64,
        );
        let g = &mut self.graph;
        *self.ids.entry(key).or_insert_with(|| {
            g.pos.push(p);
            g.adj.push(Vec::new());
            g.len.push(Vec::new());
            g.cost.push(Vec::new());
            g.pos.len() - 1
        })
    }

    /// Adds the directed edge `a -> b`, or lowers the cost of the one already there.
    fn edge(&mut self, a: usize, b: usize, len: f64, cost: f64) {
        match self.slot.get(&(a, b)) {
            Some(&i) => {
                if cost < self.graph.cost[a][i] {
                    self.graph.cost[a][i] = cost;
                    self.graph.len[a][i] = len;
                }
            }
            None => {
                self.slot.insert((a, b), self.graph.adj[a].len());
                self.graph.adj[a].push(b);
                self.graph.len[a].push(len);
                self.graph.cost[a].push(cost);
            }
        }
    }
}

impl RoadGraph {
    /// Graph of the pieces. `fixed_route` passes every piece as two-way.
    pub(super) fn build(pieces: &[Piece]) -> Self {
        let mut b = Builder {
            ids: HashMap::new(),
            graph: RoadGraph {
                pos: Vec::new(),
                adj: Vec::new(),
                len: Vec::new(),
                cost: Vec::new(),
            },
            slot: HashMap::new(),
        };
        let mut ends: Vec<usize> = Vec::new();
        for piece in pieces {
            let pts = piece.points;
            if pts.len() < 2 {
                continue;
            }
            ends.push(b.node(pts[0]));
            ends.push(b.node(pts[pts.len() - 1]));
            let mut prev = b.node(pts[0]);
            for k in 1..pts.len() {
                let cur = b.node(pts[k]);
                if prev != cur {
                    let len = dist(pts[k - 1], pts[k]);
                    let base = len / piece.speed_mps;
                    // The bend apex is the start of the edge leaving it, in each direction.
                    let forward = base + bend_penalty_s(pts, k - 1);
                    let back = base + bend_penalty_s(pts, k);
                    b.edge(prev, cur, len, forward);
                    if !piece.oneway {
                        b.edge(cur, prev, len, back);
                    }
                }
                prev = cur;
            }
        }
        // Road pieces are cut at tile edges and rounded apart, so a piece can stop a little short
        // of the next one. Endpoints within `SNAP_M` of each other are joined both ways, whatever
        // the one-way flags say: a join, not a road.
        ends.sort_unstable();
        ends.dedup();
        let pos = b.graph.pos.clone();
        let cell = |p: [f64; 2]| {
            (
                (p[0] / SNAP_M).floor() as i64,
                (p[1] / SNAP_M).floor() as i64,
            )
        };
        let mut buckets: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for &e in &ends {
            buckets.entry(cell(pos[e])).or_default().push(e);
        }
        for &e in &ends {
            let (cx, cy) = cell(pos[e]);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let Some(list) = buckets.get(&(cx + dx, cy + dy)) else {
                        continue;
                    };
                    for &f in list {
                        if f <= e || dist(pos[e], pos[f]) > SNAP_M {
                            continue;
                        }
                        let len = dist(pos[e], pos[f]);
                        let cost = len / SNAP_SPEED_MPS;
                        b.edge(e, f, len, cost);
                        b.edge(f, e, len, cost);
                    }
                }
            }
        }
        b.graph
    }

    /// The directed edge `(from, to)` nearest to `start` whose direction agrees with `heading`
    /// (dot > 0), and `start` projected on it.
    fn nearest_ahead(
        &self,
        start: [f64; 2],
        heading: [f64; 2],
    ) -> Option<(usize, usize, [f64; 2])> {
        let mut best: Option<(f64, usize, usize, [f64; 2])> = None;
        for from in 0..self.pos.len() {
            for &to in &self.adj[from] {
                let (a, b) = (self.pos[from], self.pos[to]);
                let d = sub(b, a);
                let len2 = d[0] * d[0] + d[1] * d[1];
                let t = (dot(sub(start, a), d) / len2).clamp(0.0, 1.0);
                let c = [a[0] + t * d[0], a[1] + t * d[1]];
                if dot(unit(d), heading) <= 0.0 {
                    continue;
                }
                let d2 = dist(c, start).powi(2);
                if best.map_or(true, |(b2, _, _, _)| d2 < b2) {
                    best = Some((d2, from, to, c));
                }
            }
        }
        best.map(|(_, from, to, c)| (from, to, c))
    }
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    dot(sub(a, b), sub(a, b)).sqrt()
}

fn unit(v: [f64; 2]) -> [f64; 2] {
    let len = dot(v, v).sqrt();
    if len == 0.0 {
        [0.0, 0.0]
    } else {
        [v[0] / len, v[1] / len]
    }
}

/// Time penalty, in seconds, of the bend at `pts[i]` inside one way: nothing for a straight way or
/// a gentle turn, up to `BEND_PENALTY_S` for a turn back on itself. Zero at the ends of the way.
pub(super) fn bend_penalty_s(pts: &[[f64; 2]], i: usize) -> f64 {
    if i == 0 || i + 1 >= pts.len() {
        return 0.0;
    }
    let (a, b) = (sub(pts[i], pts[i - 1]), sub(pts[i + 1], pts[i]));
    if dot(a, a) == 0.0 || dot(b, b) == 0.0 {
        return 0.0;
    }
    let turn = turn_deg(unit(a), b);
    if turn <= BEND_MIN_DEG {
        return 0.0;
    }
    BEND_PENALTY_S * (turn - BEND_MIN_DEG) / (180.0 - BEND_MIN_DEG)
}

/// Absolute angle, in degrees, between a unit direction and a vector.
fn turn_deg(dir: [f64; 2], v: [f64; 2]) -> f64 {
    let u = unit(v);
    dot(dir, u).clamp(-1.0, 1.0).acos().to_degrees()
}

/// Godot-facing helper for the drive bot: the route from a point on the saved map.
#[derive(GodotClass)]
#[class(base = RefCounted, init)]
pub struct DriveRoute {
    base: Base<RefCounted>,
}

#[godot_api]
impl DriveRoute {
    /// Waypoints `(x east, y north)` in local meters of a `length_m` route from `start` along
    /// `heading`, read from the baked map `map_id` under `cache_root`. Empty when the map cannot
    /// be read or has no road there.
    #[func]
    fn waypoints(
        &self,
        cache_root: GString,
        map_id: GString,
        start: Vector2,
        heading: Vector2,
        length_m: f64,
    ) -> PackedVector2Array {
        let root = PathBuf::from(cache_root.to_string());
        let id = map_id.to_string();
        let index = match open_map(&root, &id) {
            Ok(index) => index,
            Err(e) => {
                godot_error!("drive route: cannot open map '{id}': {e}");
                return PackedVector2Array::new();
            }
        };
        let roads = match read_roads_file(&index.roads_file()) {
            Ok(roads) => roads,
            Err(e) => {
                godot_error!("drive route: cannot read roads of '{id}': {e}");
                return PackedVector2Array::new();
            }
        };
        let start = [start.x as f64, start.y as f64];
        let reach = length_m + READ_MARGIN_M;
        let near: Vec<Vec<[f64; 2]>> = roads
            .into_iter()
            .flat_map(|(_, _, lines)| lines)
            .filter(|line| DRIVABLE_HIGHWAYS.contains(&line.highway.as_str()))
            .map(|line| line.points)
            .filter(|pts| pts.iter().any(|p| dist(*p, start) <= reach))
            .collect();
        let heading = unit([heading.x as f64, heading.y as f64]);
        fixed_route(&near, start, heading, length_m)
            .into_iter()
            .map(|p| Vector2::new(p[0] as f32, p[1] as f32))
            .collect()
    }

    /// Random route for the drive bot (item 92): origin and destination on the car roads of the baked
    /// map `map_id`, whose shortest car route is `min_m`..`max_m` long, drawn from `seed`. Returns
    /// `origin`, `destination` (Vector2, local meters), `length_m` (float) and `waypoints`
    /// (PackedVector2Array, from the origin). Empty when the map cannot be read or no route is found.
    #[func]
    fn random_route(
        &self,
        cache_root: GString,
        map_id: GString,
        seed: i64,
        min_m: f64,
        max_m: f64,
    ) -> VarDictionary {
        let mut out = VarDictionary::new();
        let root = PathBuf::from(cache_root.to_string());
        let id = map_id.to_string();
        let index = match open_map(&root, &id) {
            Ok(index) => index,
            Err(e) => {
                godot_error!("drive route: cannot open map '{id}': {e}");
                return out;
            }
        };
        let graph = match CarGraph::load(&index) {
            Ok(graph) => graph,
            Err(e) => {
                godot_error!("drive route: cannot load the car graph of '{id}': {e}");
                return out;
            }
        };
        let Some(route) = random_car_route(&graph, seed as u64, min_m, max_m) else {
            godot_error!("drive route: no {min_m}-{max_m} m route on '{id}' for seed {seed}");
            return out;
        };
        let point = |p: [f64; 2]| Vector2::new(p[0] as f32, p[1] as f32);
        let origin = point(route.points[0]);
        let destination = point(*route.points.last().unwrap());
        out.set("origin", origin);
        out.set("destination", destination);
        out.set("length_m", route.length_m);
        let waypoints: PackedVector2Array = route.points.iter().map(|&p| point(p)).collect();
        out.set("waypoints", &waypoints);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A straight east-west road and a north branch at x = 100: the route going east keeps straight.
    #[test]
    fn keeps_straight_at_a_junction() {
        let east: Vec<[f64; 2]> = (0..=300).map(|i| [i as f64, 0.0]).collect();
        let north: Vec<[f64; 2]> = (0..=100).map(|i| [100.0, i as f64]).collect();
        let route = fixed_route(&[east, north], [0.0, 0.0], [1.0, 0.0], 250.0);
        assert!(route.len() > 2);
        assert!(
            route.iter().all(|p| p[1].abs() < 1e-9),
            "left the straight road: {route:?}"
        );
        let end = route.last().unwrap();
        assert!(end[0] >= 250.0 - 1e-6, "too short: {end:?}");
    }

    /// Dead end: the route stops there instead of inventing a way on.
    /// Random route (item 92) on a 3 km grid of residential roads, 100 m apart: the route is in range,
    /// the same seed gives the same route and other seeds give other ones.
    #[test]
    fn random_route_is_in_range_and_repeats_for_a_seed() {
        use tileformat::Polyline;
        let line = |points: Vec<[f64; 2]>| Polyline {
            highway: "residential".into(),
            name: String::new(),
            points,
            z: vec![],
            zl: vec![],
            zr: vec![],
            lit: None,
            oneway: None,
            lanes: None,
            maxspeed: None,
            roundabout: false,
        };
        let steps: Vec<f64> = (0..=30).map(|i| i as f64 * 100.0).collect();
        let mut lines = Vec::new();
        for &y in &steps {
            lines.push(line(steps.iter().map(|&x| [x, y]).collect()));
            lines.push(line(steps.iter().map(|&x| [y, x]).collect()));
        }
        let graph = CarGraph::from_polylines(lines);
        let a = random_car_route(&graph, 7, 950.0, 1050.0).expect("a route");
        assert!(
            (950.0..=1050.0).contains(&a.length_m),
            "length {}",
            a.length_m
        );
        assert_eq!(a, random_car_route(&graph, 7, 950.0, 1050.0).unwrap());
        let others: Vec<_> = (1..=5)
            .map(|s| random_car_route(&graph, s, 950.0, 1050.0).unwrap().points)
            .collect();
        assert!(
            others.iter().any(|p| *p != a.points),
            "all seeds gave one route"
        );
    }

    #[test]
    fn stops_at_a_dead_end() {
        let road: Vec<[f64; 2]> = (0..=50).map(|i| [i as f64, 0.0]).collect();
        let route = fixed_route(&[road], [0.0, 0.0], [1.0, 0.0], 500.0);
        assert!((route.last().unwrap()[0] - 50.0).abs() < 1e-9);
    }
}
