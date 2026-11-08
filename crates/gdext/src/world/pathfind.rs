//! Car routing on the road graph (item 91): the shortest way between two nodes of the drivable
//! roads, by A* with a binary heap and ALT landmarks (Goldberg and Harrelson). The cost of an edge
//! is its travel time: length over the speed of the way (its `maxspeed` when it reads as a number,
//! else the speed of its highway class), plus the bend penalty of `route.rs`.
//!
//! Only car roads are in the graph (`DRIVABLE_HIGHWAYS`). A one-way way is walked from its first
//! point to its last only. Landmarks (16, chosen farthest-first) and their distances are computed
//! once, when the graph is built from the tiles; no other preprocessing and no new file.
//!
//! Known limits, kept on purpose:
//! - `oneway=-1` and `reversible` read as two-way: a tile keeps no value for them (`oneway: None`).
//! - `junction=roundabout` is one-way in the direction of its way, as OSM means it.
//! - Turn restrictions are not modelled; the bend penalty is charged inside one way only.
//! - Snap joins between road ends (tile cuts) are two-way whatever the one-way flags say.
#![cfg_attr(not(test), allow(dead_code))] // Used by the GPS (item 93) and the drive bot (item 92).

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io;

use tileformat::Polyline;

use super::map::MapIndex;
use super::route::{Piece, RoadGraph, DRIVABLE_HIGHWAYS};

/// Landmarks per graph (ALT); fewer when the graph has fewer nodes.
const LANDMARKS: usize = 16;
/// Speeds read below this, in km/h, count as this one.
const MIN_SPEED_KMH: f64 = 5.0;

/// Speed of a highway class, in km/h, when the way has no readable `maxspeed`.
fn class_speed_kmh(highway: &str) -> f64 {
    match highway {
        "motorway" => 110.0,
        "motorway_link" => 60.0,
        "trunk" => 90.0,
        "trunk_link" => 50.0,
        "primary" => 70.0,
        "primary_link" => 40.0,
        "secondary" => 60.0,
        "secondary_link" => 40.0,
        "tertiary" => 50.0,
        "tertiary_link" => 35.0,
        "unclassified" | "road" => 40.0,
        "residential" => 30.0,
        "living_street" | "service" => 15.0,
        "track" => 20.0,
        _ => 30.0,
    }
}

/// Speed of a `maxspeed` tag, in km/h: "50" or "50 km/h" are km/h, "30 mph" is miles. `None` for
/// anything else ("none", "walk", "IT:urban", "variable", ...), and for zero or negative values.
pub fn parse_maxspeed_kmh(raw: &str) -> Option<f64> {
    let s = raw.trim();
    let (num, factor) = if let Some(n) = s.strip_suffix("mph") {
        (n, 1.609_344)
    } else if let Some(n) = s.strip_suffix("km/h") {
        (n, 1.0)
    } else {
        (s, 1.0)
    };
    let v: f64 = num.trim().parse().ok()?;
    (v.is_finite() && v > 0.0).then_some(v * factor)
}

/// Speed of a way in m/s: its `maxspeed` when it reads, else its class; never below `MIN_SPEED_KMH`.
fn speed_mps(line: &Polyline) -> f64 {
    let kmh = line
        .maxspeed
        .as_deref()
        .and_then(parse_maxspeed_kmh)
        .unwrap_or_else(|| class_speed_kmh(&line.highway));
    kmh.max(MIN_SPEED_KMH) / 3.6
}

/// Travel times from one landmark: `from[v]` is the time from the landmark to `v`, `to[v]` the time
/// from `v` back to it. Infinite where there is no way.
struct Landmark {
    from: Vec<f64>,
    to: Vec<f64>,
}

/// Car road graph with the ALT landmarks of its nodes.
pub struct CarGraph {
    graph: RoadGraph,
    /// `rev[v]`: `(u, time, length)` of every edge `u -> v`, for the searches from a landmark's side.
    rev: Vec<Vec<(usize, f64, f64)>>,
    landmarks: Vec<Landmark>,
}

/// A route: its points in order (graph nodes, the first is the start), its length in meters, its
/// travel time in seconds, and how many nodes the search took out of the heap (`expanded`).
#[derive(Debug, Clone, PartialEq)]
pub struct CarRoute {
    pub points: Vec<[f64; 2]>,
    pub length_m: f64,
    pub time_s: f64,
    pub expanded: usize,
}

/// `Alt` is the search of the game; `Dijkstra` runs the same code with no heuristic, for reference
/// and for comparing the expanded nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Alt,
    Dijkstra,
}

fn relax(dist: &mut [f64], heap: &mut BinaryHeap<Reverse<(u64, usize)>>, v: usize, nd: f64) {
    if nd < dist[v] {
        dist[v] = nd;
        // Times are never negative, so their bit patterns order like the numbers.
        heap.push(Reverse((nd.to_bits(), v)));
    }
}

impl CarGraph {
    /// Graph of the car roads of the given road pieces (tile polylines, names and tags kept).
    pub fn from_polylines(lines: Vec<Polyline>) -> Self {
        let roads: Vec<(Vec<[f64; 2]>, bool, f64)> = lines
            .into_iter()
            .filter(|l| DRIVABLE_HIGHWAYS.contains(&l.highway.as_str()))
            .map(|l| {
                let speed = speed_mps(&l);
                // Roundabouts are one-way in OSM even when they carry no `oneway` tag.
                let oneway = l.oneway == Some(true) || l.roundabout;
                (l.points, oneway, speed)
            })
            .collect();
        let pieces: Vec<Piece> = roads
            .iter()
            .map(|(points, oneway, speed_mps)| Piece {
                points,
                oneway: *oneway,
                speed_mps: *speed_mps,
            })
            .collect();
        Self::from_pieces(&pieces)
    }

    /// Graph of every tile of the map `index`. Reads the tile files (not `roads.bin`, which keeps no
    /// tags); the cache is only read.
    pub fn load(index: &MapIndex) -> io::Result<Self> {
        let mut lines = Vec::new();
        for &(x, y) in index.coords() {
            lines.extend(index.read_roads_at(x, y)?);
        }
        Ok(Self::from_polylines(lines))
    }

    fn from_pieces(pieces: &[Piece]) -> Self {
        let graph = RoadGraph::build(pieces);
        let n = graph.pos.len();
        let mut rev = vec![Vec::new(); n];
        for u in 0..n {
            for (i, &v) in graph.adj[u].iter().enumerate() {
                rev[v].push((u, graph.cost[u][i], graph.len[u][i]));
            }
        }
        let mut g = CarGraph {
            graph,
            rev,
            landmarks: Vec::new(),
        };
        g.landmarks = g.select_landmarks();
        g
    }

    pub fn node_count(&self) -> usize {
        self.graph.pos.len()
    }

    pub fn landmark_count(&self) -> usize {
        self.landmarks.len()
    }

    /// Position of node `u`, in the local meters of the bake.
    pub fn position(&self, u: usize) -> [f64; 2] {
        self.graph.pos[u]
    }

    /// Shortest car route from node `from` to node `to`, by A* with ALT. `None` when `to` cannot be
    /// reached (one-way streets respected) or a node is out of range.
    pub fn shortest_path(&self, from: usize, to: usize) -> Option<CarRoute> {
        self.search(from, to, Mode::Alt)
    }

    pub fn shortest_path_with(&self, from: usize, to: usize, mode: Mode) -> Option<CarRoute> {
        self.search(from, to, mode)
    }

    /// Dijkstra from `src` over the edges (or, when `reverse`, the edges backwards): the time and the
    /// length of the best way to, or from, every node. Infinite time where there is no way.
    fn sssp(&self, src: usize, reverse: bool) -> (Vec<f64>, Vec<f64>) {
        let n = self.node_count();
        let mut time = vec![f64::INFINITY; n];
        let mut len = vec![0.0; n];
        let mut heap = BinaryHeap::new();
        time[src] = 0.0;
        heap.push(Reverse((0u64, src)));
        while let Some(Reverse((key, u))) = heap.pop() {
            let tu = f64::from_bits(key);
            if tu > time[u] {
                continue;
            }
            if reverse {
                for &(v, w, l) in &self.rev[u] {
                    if tu + w < time[v] {
                        len[v] = len[u] + l;
                        relax(&mut time, &mut heap, v, tu + w);
                    }
                }
            } else {
                for (i, &v) in self.graph.adj[u].iter().enumerate() {
                    let w = self.graph.cost[u][i];
                    if tu + w < time[v] {
                        len[v] = len[u] + self.graph.len[u][i];
                        relax(&mut time, &mut heap, v, tu + w);
                    }
                }
            }
        }
        (time, len)
    }

    /// Farthest-first landmarks: the first is node 0, each next one is the node whose nearest
    /// landmark (either way) is farthest. Stops at `LANDMARKS`, or when every node is a landmark.
    fn select_landmarks(&self) -> Vec<Landmark> {
        let n = self.node_count();
        let wanted = LANDMARKS.min(n);
        let mut out = Vec::new();
        if wanted == 0 {
            return out;
        }
        // Time to the nearest landmark so far; infinite where no landmark reaches the node.
        let mut near = vec![f64::INFINITY; n];
        let mut next = 0;
        loop {
            let (from, _) = self.sssp(next, false);
            let (to, _) = self.sssp(next, true);
            for v in 0..n {
                near[v] = near[v].min(from[v]).min(to[v]);
            }
            out.push(Landmark { from, to });
            if out.len() == wanted {
                break;
            }
            let (best, far) =
                near.iter().enumerate().fold(
                    (0, -1.0),
                    |acc, (v, &d)| if d > acc.1 { (v, d) } else { acc },
                );
            if far <= 0.0 {
                break;
            }
            next = best;
        }
        out
    }

    /// Lower bound of the time from `v` to `t`: the largest triangle-inequality bound of the
    /// landmarks, using only the distances that are finite. Zero when there is none.
    fn heuristic(&self, v: usize, t: usize) -> f64 {
        let mut h = 0.0_f64;
        for l in &self.landmarks {
            let (fv, ft) = (l.from[v], l.from[t]);
            if fv.is_finite() && ft.is_finite() {
                h = h.max(ft - fv);
            }
            let (tv, tt) = (l.to[v], l.to[t]);
            if tv.is_finite() && tt.is_finite() {
                h = h.max(tv - tt);
            }
        }
        h
    }

    fn search(&self, from: usize, to: usize, mode: Mode) -> Option<CarRoute> {
        let n = self.node_count();
        if from >= n || to >= n {
            return None;
        }
        let h = |v: usize| match mode {
            Mode::Alt => self.heuristic(v, to),
            Mode::Dijkstra => 0.0,
        };
        let mut time = vec![f64::INFINITY; n];
        let mut len = vec![0.0; n];
        let mut prev = vec![usize::MAX; n];
        let mut closed = vec![false; n];
        let mut heap = BinaryHeap::new();
        time[from] = 0.0;
        heap.push(Reverse((h(from).to_bits(), from)));
        let mut expanded = 0;
        while let Some(Reverse((_, u))) = heap.pop() {
            if closed[u] {
                continue;
            }
            closed[u] = true;
            expanded += 1;
            if u == to {
                break;
            }
            for (i, &v) in self.graph.adj[u].iter().enumerate() {
                if closed[v] {
                    continue;
                }
                let nt = time[u] + self.graph.cost[u][i];
                if nt < time[v] {
                    time[v] = nt;
                    len[v] = len[u] + self.graph.len[u][i];
                    prev[v] = u;
                    heap.push(Reverse(((nt + h(v)).to_bits(), v)));
                }
            }
        }
        if !closed[to] {
            return None;
        }
        let mut nodes = vec![to];
        let mut cur = to;
        while cur != from {
            cur = prev[cur];
            nodes.push(cur);
        }
        nodes.reverse();
        Some(CarRoute {
            points: nodes.iter().map(|&u| self.graph.pos[u]).collect(),
            length_m: len[to],
            time_s: time[to],
            expanded,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;
    use std::time::Instant;

    use super::super::map::open_map;
    use super::*;

    /// Small xorshift generator, so the tests need no crate.
    struct Rng(u64);

    impl Rng {
        fn next_u64(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next_u64() % n as u64) as usize
        }
        fn chance(&mut self, p: f64) -> bool {
            ((self.next_u64() >> 11) as f64 / (1u64 << 53) as f64) < p
        }
    }

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

    /// Node of `g` at `p` (exact position), for the tests that name points.
    fn node_at(g: &CarGraph, p: [f64; 2]) -> usize {
        (0..g.node_count())
            .find(|&u| g.position(u) == p)
            .unwrap_or_else(|| panic!("no node at {p:?}"))
    }

    /// Grid of `side` x `side` nodes 100 m apart; each street is a two-point piece
    /// `(points, oneway, speed in m/s)`. Some streets are dropped, some are one-way, speeds vary.
    fn random_net(rng: &mut Rng, side: usize) -> Vec<(Vec<[f64; 2]>, bool, f64)> {
        let speeds = [5.0, 8.3, 13.9, 22.2];
        let mut net = Vec::new();
        for i in 0..side {
            for j in 0..side {
                let mut streets = Vec::new();
                if i + 1 < side {
                    streets.push(((i, j), (i + 1, j)));
                }
                if j + 1 < side {
                    streets.push(((i, j), (i, j + 1)));
                }
                for (a, b) in streets {
                    if rng.chance(0.1) {
                        continue;
                    }
                    let oneway = rng.chance(0.3);
                    let (a, b) = if rng.chance(0.5) { (b, a) } else { (a, b) };
                    let pt = |(i, j): (usize, usize)| [i as f64 * 100.0, j as f64 * 100.0];
                    let speed = speeds[rng.below(speeds.len())];
                    net.push((vec![pt(a), pt(b)], oneway, speed));
                }
            }
        }
        net
    }

    fn pieces(net: &[(Vec<[f64; 2]>, bool, f64)]) -> Vec<Piece<'_>> {
        net.iter()
            .map(|(points, oneway, speed_mps)| Piece {
                points,
                oneway: *oneway,
                speed_mps: *speed_mps,
            })
            .collect()
    }

    /// Reference: directed edges `(from, to) -> time` of the net, by grid index, built apart from
    /// `CarGraph`. A two-way street gives both directions.
    fn reference_edges(
        net: &[(Vec<[f64; 2]>, bool, f64)],
        side: usize,
    ) -> HashMap<(usize, usize), f64> {
        let id =
            |p: [f64; 2]| (p[0] / 100.0).round() as usize * side + (p[1] / 100.0).round() as usize;
        let mut edges: HashMap<(usize, usize), f64> = HashMap::new();
        let mut put = |a: usize, b: usize, t: f64| {
            let e = edges.entry((a, b)).or_insert(f64::INFINITY);
            *e = e.min(t);
        };
        for (pts, oneway, speed) in net {
            let (a, b) = (id(pts[0]), id(pts[1]));
            let t = dist(pts[0], pts[1]) / speed;
            put(a, b, t);
            if !oneway {
                put(b, a, t);
            }
        }
        edges
    }

    fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
    }

    /// Reference: O(V^2) Dijkstra over the reference edges. `None` when unreachable.
    fn reference_time(
        edges: &HashMap<(usize, usize), f64>,
        n: usize,
        src: usize,
        dst: usize,
    ) -> Option<f64> {
        let mut best = vec![f64::INFINITY; n];
        let mut done = vec![false; n];
        best[src] = 0.0;
        loop {
            let u = (0..n)
                .filter(|&v| !done[v] && best[v].is_finite())
                .min_by(|&a, &b| best[a].total_cmp(&best[b]));
            let Some(u) = u else { break };
            done[u] = true;
            for (&(a, b), &t) in edges {
                if a == u && best[u] + t < best[b] {
                    best[b] = best[u] + t;
                }
            }
        }
        best[dst].is_finite().then_some(best[dst])
    }

    #[test]
    fn parses_maxspeed() {
        assert_eq!(parse_maxspeed_kmh("50"), Some(50.0));
        assert_eq!(parse_maxspeed_kmh("50 km/h"), Some(50.0));
        assert!((parse_maxspeed_kmh("30 mph").unwrap() - 48.28032).abs() < 1e-9);
        for raw in ["none", "walk", "IT:urban", "variable", "", "0", "-5"] {
            assert_eq!(parse_maxspeed_kmh(raw), None, "{raw:?}");
        }
    }

    /// Speed: a readable `maxspeed` wins over the class; without it the class speed counts.
    #[test]
    fn speed_uses_maxspeed_then_class() {
        let a = [0.0, 0.0];
        let b = [300.0, 0.0];
        let mut with_limit = line("residential", &[a, b]);
        with_limit.maxspeed = Some("50".into());
        let mut junk = line("residential", &[a, b]);
        junk.maxspeed = Some("none".into());
        let g = CarGraph::from_polylines(vec![with_limit]);
        let t = g
            .shortest_path(node_at(&g, a), node_at(&g, b))
            .unwrap()
            .time_s;
        assert!((t - 300.0 / (50.0 / 3.6)).abs() < 1e-9, "t = {t}");
        let g = CarGraph::from_polylines(vec![junk]);
        let t = g
            .shortest_path(node_at(&g, a), node_at(&g, b))
            .unwrap()
            .time_s;
        assert!((t - 300.0 / (30.0 / 3.6)).abs() < 1e-9, "t = {t}");
    }

    /// Non-car ways are never routed on: a footway that reaches `b` does not connect `a` to it.
    #[test]
    fn footways_are_not_car_roads() {
        let (a, b, c) = ([0.0, 0.0], [100.0, 0.0], [0.0, 100.0]);
        let g =
            CarGraph::from_polylines(vec![line("footway", &[a, b]), line("residential", &[a, c])]);
        assert_eq!(g.node_count(), 2, "b is only on the footway");
        assert!(g.shortest_path(node_at(&g, a), node_at(&g, c)).is_some());
        let footway_only = CarGraph::from_polylines(vec![line("footway", &[a, b])]);
        assert_eq!(footway_only.node_count(), 0);
    }

    /// A one-way way goes its way only; the reverse needs a way around.
    #[test]
    fn oneway_is_respected() {
        let (a, b, c, d) = ([0.0, 0.0], [100.0, 0.0], [200.0, 0.0], [100.0, -100.0]);
        let mut oneway = line("primary", &[a, b, c]);
        oneway.oneway = Some(true);
        let mut back = line("primary", &[a, d, c]);
        back.oneway = Some(false);
        let g = CarGraph::from_polylines(vec![oneway, back]);
        let (ia, ic) = (node_at(&g, a), node_at(&g, c));
        let direct = g.shortest_path(ia, ic).unwrap();
        assert_eq!(direct.points, vec![a, b, c]);
        assert!((direct.length_m - 200.0).abs() < 1e-9);
        let around = g.shortest_path(ic, ia).unwrap();
        assert_eq!(around.points, vec![c, d, a]);
        assert!((around.length_m - 2.0 * dist(c, d)).abs() < 1e-9);

        let single = CarGraph::from_polylines(vec![{
            let mut l = line("primary", &[a, b]);
            l.oneway = Some(true);
            l
        }]);
        assert!(single
            .shortest_path(node_at(&single, a), node_at(&single, b))
            .is_some());
        assert!(single
            .shortest_path(node_at(&single, b), node_at(&single, a))
            .is_none());
    }

    /// Roundabouts are one-way in the way's direction.
    #[test]
    fn roundabout_is_one_way() {
        let (a, b) = ([0.0, 0.0], [50.0, 0.0]);
        let mut round = line("tertiary", &[a, b]);
        round.roundabout = true;
        let g = CarGraph::from_polylines(vec![round]);
        assert!(g.shortest_path(node_at(&g, a), node_at(&g, b)).is_some());
        assert!(g.shortest_path(node_at(&g, b), node_at(&g, a)).is_none());
    }

    /// Two parts with no road between them: `None`, from either side.
    #[test]
    fn unreachable_is_none() {
        let g = CarGraph::from_polylines(vec![
            line("residential", &[[0.0, 0.0], [100.0, 0.0]]),
            line("residential", &[[0.0, 500.0], [100.0, 500.0]]),
        ]);
        let (a, b) = (node_at(&g, [0.0, 0.0]), node_at(&g, [0.0, 500.0]));
        assert!(g.shortest_path(a, b).is_none());
        assert!(g.shortest_path(b, a).is_none());
        assert!(g.shortest_path(a, a).is_some());
        assert!(g.shortest_path(a, g.node_count()).is_none());
    }

    /// A bend inside one way adds its penalty: a 90 degree apex costs 4 s * (90 - 30) / (180 - 30).
    #[test]
    fn bend_adds_time_both_ways() {
        let (a, b, c) = ([0.0, 0.0], [100.0, 0.0], [100.0, 100.0]);
        let g = CarGraph::from_polylines(vec![line("residential", &[a, b, c])]);
        let penalty = 4.0 * 60.0 / 150.0;
        let speed = 30.0 / 3.6;
        let fwd = g.shortest_path(node_at(&g, a), node_at(&g, c)).unwrap();
        assert!(
            (fwd.time_s - (200.0 / speed + penalty)).abs() < 1e-9,
            "{}",
            fwd.time_s
        );
        let back = g.shortest_path(node_at(&g, c), node_at(&g, a)).unwrap();
        assert!(
            (back.time_s - (200.0 / speed + penalty)).abs() < 1e-9,
            "{}",
            back.time_s
        );
    }

    /// Random graphs: ALT agrees with the reference Dijkstra on 200 random pairs, its routes are
    /// real routes (each step a legal edge, the time the sum of their edges), and unreachable
    /// pairs give `None`.
    #[test]
    fn matches_dijkstra_on_random_pairs() {
        let side = 12;
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let net = random_net(&mut rng, side);
        let edges = reference_edges(&net, side);
        let ps = pieces(&net);
        let g = CarGraph::from_pieces(&ps);
        let n = g.node_count();
        let grid = |u: usize| {
            let p = g.position(u);
            (p[0] / 100.0).round() as usize * side + (p[1] / 100.0).round() as usize
        };
        let (mut reachable, mut expanded_alt, mut expanded_dij) = (0usize, 0usize, 0usize);
        for _ in 0..200 {
            let (src, dst) = (rng.below(n), rng.below(n));
            let expect = reference_time(&edges, side * side, grid(src), grid(dst));
            let alt = g.shortest_path(src, dst);
            let dij = g.shortest_path_with(src, dst, Mode::Dijkstra);
            match expect {
                None => {
                    assert!(
                        alt.is_none() && dij.is_none(),
                        "{src} -> {dst} should be unreachable"
                    );
                }
                Some(t) => {
                    reachable += 1;
                    let alt = alt.unwrap_or_else(|| panic!("{src} -> {dst} lost"));
                    let dij = dij.unwrap();
                    let tol = 1e-9 * t.max(1.0);
                    assert!(
                        (alt.time_s - t).abs() <= tol,
                        "alt {} vs ref {t}",
                        alt.time_s
                    );
                    assert!(
                        (dij.time_s - t).abs() <= tol,
                        "dijkstra {} vs ref {t}",
                        dij.time_s
                    );
                    let mut sum_t = 0.0;
                    for w in alt.points.windows(2) {
                        let key = (
                            (w[0][0] / 100.0).round() as usize * side
                                + (w[0][1] / 100.0).round() as usize,
                            (w[1][0] / 100.0).round() as usize * side
                                + (w[1][1] / 100.0).round() as usize,
                        );
                        sum_t += *edges
                            .get(&key)
                            .unwrap_or_else(|| panic!("illegal step {key:?}"));
                    }
                    assert!((sum_t - t).abs() <= tol, "path time {sum_t} vs ref {t}");
                    assert_eq!(alt.points.first().copied(), Some(g.position(src)));
                    assert_eq!(alt.points.last().copied(), Some(g.position(dst)));
                    expanded_alt += alt.expanded;
                    expanded_dij += dij.expanded;
                }
            }
        }
        println!(
            "random grid: {n} nodes, {} landmarks, {reachable}/200 reachable pairs; expanded ALT {expanded_alt}, Dijkstra {expanded_dij}",
            g.landmark_count()
        );
        assert!(reachable >= 100, "too few reachable pairs: {reachable}");
    }

    fn median(mut v: Vec<f64>) -> f64 {
        v.sort_by(|a, b| a.total_cmp(b));
        v[v.len() / 2]
    }

    /// `casa-10km` (cache read only): time of the queries of about 1 km and about 8 km, and the
    /// expanded nodes of ALT against Dijkstra. Run with `--ignored --nocapture`.
    #[test]
    #[ignore]
    fn casa_10km_query_times() {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../cache"));
        let index = open_map(root, "casa-10km").unwrap();
        let t0 = Instant::now();
        let g = CarGraph::load(&index).unwrap();
        println!(
            "load: {:.2} s, {} nodes, {} landmarks",
            t0.elapsed().as_secs_f64(),
            g.node_count(),
            g.landmark_count()
        );
        let n = g.node_count();
        let mut rng = Rng(91);
        for (lo, hi, label) in [(950.0, 1050.0, "1 km"), (7600.0, 8400.0, "8 km")] {
            let (mut t_alt, mut t_dij, mut x_alt, mut x_dij) = (vec![], vec![], vec![], vec![]);
            let mut tries = 0;
            while t_alt.len() < 30 && tries < 300 {
                tries += 1;
                // Full Dijkstra tree from a random source gives the length of the best way to each node.
                let src = rng.below(n);
                let (time, len) = g.sssp(src, false);
                let targets: Vec<usize> = (0..n)
                    .filter(|&v| len[v] >= lo && len[v] <= hi && time[v].is_finite())
                    .collect();
                if targets.is_empty() {
                    continue;
                }
                let dst = targets[rng.below(targets.len())];
                let t = Instant::now();
                let alt = g.shortest_path(src, dst).unwrap();
                t_alt.push(t.elapsed().as_secs_f64() * 1e3);
                let t = Instant::now();
                let dij = g.shortest_path_with(src, dst, Mode::Dijkstra).unwrap();
                t_dij.push(t.elapsed().as_secs_f64() * 1e3);
                assert!((alt.time_s - dij.time_s).abs() < 1e-6);
                assert!(alt.length_m >= lo - 1e-6 && alt.length_m <= hi + 1e-6);
                x_alt.push(alt.expanded as f64);
                x_dij.push(dij.expanded as f64);
            }
            println!(
                "{label}: {} queries (length {lo}-{hi} m), median ms ALT {:.3} / Dijkstra {:.3}, median expanded ALT {} / Dijkstra {}",
                t_alt.len(),
                median(t_alt),
                median(t_dij),
                median(x_alt) as usize,
                median(x_dij) as usize,
            );
        }
    }
}
