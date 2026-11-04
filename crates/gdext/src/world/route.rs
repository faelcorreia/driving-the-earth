//! Fixed driving route for the drive bot (item 4). From a start point and heading on the map,
//! walks the road graph and keeps the straightest way at every junction, so the same start always
//! gives the same route. Local meters: `x` east, `y` north (see `world::map`).
//!
//! The graph is built from the road pieces of `roads.bin`: each consecutive pair of points is an
//! edge, both ways, and points closer than `NODE_GRID_M` are the same node.
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use godot::prelude::*;
use tileformat::roads::read_roads_file;

use super::map::open_map;

/// Points within this distance, in meters, are the same graph node.
const NODE_GRID_M: f64 = 0.01;
/// OSM `highway` values a car can drive on. Footways and paths are left out: a footway piece can
/// end where the road beside it goes on, and the route would stop there.
const DRIVABLE_HIGHWAYS: &[&str] = &[
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

/// Walks the road graph from `start` along `heading` (unit vector) for `length_m` meters and
/// returns the points of the route, the first one being `start` projected on the first road
/// segment. Ends early at a dead end.
pub fn fixed_route(roads: &[Vec<[f64; 2]>], start: [f64; 2], heading: [f64; 2], length_m: f64) -> Vec<[f64; 2]> {
    let graph = RoadGraph::build(roads);
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

/// Road graph: nodes are the quantized points, edges go both ways between consecutive points.
struct RoadGraph {
    pos: Vec<[f64; 2]>,
    adj: Vec<Vec<usize>>,
}

impl RoadGraph {
    fn build(roads: &[Vec<[f64; 2]>]) -> Self {
        let mut ids: HashMap<(i64, i64), usize> = HashMap::new();
        let mut pos: Vec<[f64; 2]> = Vec::new();
        let mut adj: Vec<Vec<usize>> = Vec::new();
        let mut edges: HashSet<(usize, usize)> = HashSet::new();
        let mut node = |p: [f64; 2], pos: &mut Vec<[f64; 2]>, adj: &mut Vec<Vec<usize>>| -> usize {
            let key = (
                (p[0] / NODE_GRID_M).round() as i64,
                (p[1] / NODE_GRID_M).round() as i64,
            );
            *ids.entry(key).or_insert_with(|| {
                pos.push(p);
                adj.push(Vec::new());
                pos.len() - 1
            })
        };
        let mut ends: Vec<usize> = Vec::new();
        for road in roads {
            if road.len() < 2 {
                continue;
            }
            ends.push(node(road[0], &mut pos, &mut adj));
            ends.push(node(road[road.len() - 1], &mut pos, &mut adj));
            for seg in road.windows(2) {
                let a = node(seg[0], &mut pos, &mut adj);
                let b = node(seg[1], &mut pos, &mut adj);
                if a == b {
                    continue;
                }
                if edges.insert((a, b)) {
                    adj[a].push(b);
                }
                if edges.insert((b, a)) {
                    adj[b].push(a);
                }
            }
        }
        // Road pieces are cut at tile edges and rounded apart, so a piece can stop a little short
        // of the next one. Endpoints within `SNAP_M` of each other are joined both ways.
        ends.sort_unstable();
        ends.dedup();
        let cell = |p: [f64; 2]| ((p[0] / SNAP_M).floor() as i64, (p[1] / SNAP_M).floor() as i64);
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
                        if edges.insert((e, f)) {
                            adj[e].push(f);
                        }
                        if edges.insert((f, e)) {
                            adj[f].push(e);
                        }
                    }
                }
            }
        }
        RoadGraph { pos, adj }
    }

    /// The directed edge `(from, to)` nearest to `start` whose direction agrees with `heading`
    /// (dot > 0), and `start` projected on it.
    fn nearest_ahead(&self, start: [f64; 2], heading: [f64; 2]) -> Option<(usize, usize, [f64; 2])> {
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
        assert!(route.iter().all(|p| p[1].abs() < 1e-9), "left the straight road: {route:?}");
        let end = route.last().unwrap();
        assert!(end[0] >= 250.0 - 1e-6, "too short: {end:?}");
    }

    /// Dead end: the route stops there instead of inventing a way on.
    #[test]
    fn stops_at_a_dead_end() {
        let road: Vec<[f64; 2]> = (0..=50).map(|i| [i as f64, 0.0]).collect();
        let route = fixed_route(&[road], [0.0, 0.0], [1.0, 0.0], 500.0);
        assert!((route.last().unwrap()[0] - 50.0).abs() < 1e-9);
    }
}
