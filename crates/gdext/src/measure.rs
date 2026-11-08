//! F10a baseline (run by hand, not in `cargo test`): triangles per tile and `tile_geometry` time.
//! Run: `cargo test --release -p driving_the_earth_ext measure -- --ignored --nocapture`.
use std::path::Path;
use std::time::Instant;

use std::collections::HashMap;

use crate::geometry::TileGeometry;
use crate::world::open_map;
use crate::world::spawn::find_spawn;

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

#[test]
#[ignore]
fn baseline_casa() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache");
    let id = std::env::var("MEASURE_MAP").unwrap_or_else(|_| "casa".into());
    let index = open_map(&root, &id).expect("open map");
    let (mut road, mut terr, mut bld, mut ms) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut lamps = Vec::new();
    for &(x, y) in index.coords() {
        let t0 = Instant::now();
        let g = index.tile_geometry(x, y).expect("tile_geometry");
        ms.push(t0.elapsed().as_secs_f64() * 1e3);
        road.push((g.roads.indices.len() / 3) as f64);
        terr.push((g.terrain.indices.len() / 3) as f64);
        bld.push((g.buildings.indices.len() / 3) as f64);
        lamps.push(g.lamps.len() as f64);
    }
    let mut all: Vec<f64> = road.iter().zip(&terr).map(|(a, b)| a + b).collect();
    println!("map {id}: {} tiles", ms.len());
    println!(
        "road tri p50 {:.0} p90 {:.0}",
        pct(&mut road.clone(), 0.5),
        pct(&mut road.clone(), 0.9)
    );
    println!(
        "terrain tri p50 {:.0} p90 {:.0}",
        pct(&mut terr.clone(), 0.5),
        pct(&mut terr.clone(), 0.9)
    );
    println!(
        "building tri p50 {:.0} p90 {:.0}",
        pct(&mut bld.clone(), 0.5),
        pct(&mut bld.clone(), 0.9)
    );
    println!(
        "total tri p50 {:.0} p90 {:.0}",
        pct(&mut all.clone(), 0.5),
        pct(&mut all, 0.9)
    );
    println!(
        "lamps per tile p50 {:.0} p90 {:.0} (total {})",
        pct(&mut lamps.clone(), 0.5),
        pct(&mut lamps.clone(), 0.9),
        lamps.iter().sum::<f64>()
    );
    println!(
        "tile_geometry ms p50 {:.2} p90 {:.2}",
        pct(&mut ms.clone(), 0.5),
        pct(&mut ms, 0.9)
    );
}

/// Rendered terrain height at `p` (map meters) from the triangles of tile `key`'s terrain mesh.
fn terrain_h(g: &TileGeometry, tm: f64, key: (i32, i32), p: [f64; 2]) -> f64 {
    let v = &g.terrain.vertices;
    let n = (v.len() as f64).sqrt().round() as usize;
    let cell = tm / (n - 1) as f64;
    let fx = ((p[0] - key.0 as f64 * tm) / cell).clamp(0.0, (n - 1) as f64 - 1e-9);
    let fy = ((p[1] - key.1 as f64 * tm) / cell).clamp(0.0, (n - 1) as f64 - 1e-9);
    let (j, i) = (fx.floor() as usize, fy.floor() as usize);
    let (u, w) = (fx - j as f64, fy - i as f64);
    let h = |r: usize, c: usize| v[r * n + c][1] as f64;
    let (a, b, c, d) = (h(i, j), h(i, j + 1), h(i + 1, j), h(i + 1, j + 1));
    // Same split as the mesh: (a, b, c) for u + w <= 1, else (d, b, c).
    if u + w <= 1.0 {
        a * (1.0 - u - w) + b * u + c * w
    } else {
        d * (u + w - 1.0) + b * (1.0 - w) + c * (1.0 - u)
    }
}

/// Terrain height under `p` from the tile that owns it, `None` when no tile covers `p`.
fn terrain_at(geo: &HashMap<(i32, i32), TileGeometry>, tm: f64, p: [f64; 2]) -> Option<f64> {
    let key = ((p[0] / tm).floor() as i32, (p[1] / tm).floor() as i32);
    geo.get(&key).map(|g| terrain_h(g, tm, key, p))
}

/// One road station: rendered axis and edge heights minus the rendered terrain under each.
struct Station {
    pos: [f64; 2],
    d_axis: f64,
    d_edge: [f64; 2],
    /// Ribbon edge minus axis (what zl/zr add to the ribbon), per edge.
    rib: [f64; 2],
    /// Terrain under the edge minus terrain under the axis (the cross slope), per edge.
    ter: [f64; 2],
}

fn summarize(name: &str, st: &[&Station]) {
    if st.is_empty() {
        println!("[{name}] no stations");
        return;
    }
    let axis: Vec<f64> = st.iter().map(|s| s.d_axis).collect();
    let edge: Vec<f64> = st.iter().flat_map(|s| s.d_edge).collect();
    let mut rib: Vec<f64> = st.iter().flat_map(|s| s.rib.map(f64::abs)).collect();
    let mut ter: Vec<f64> = st.iter().flat_map(|s| s.ter.map(f64::abs)).collect();
    let mut no_lift: Vec<f64> = st.iter().map(|s| s.d_axis - 0.1).collect();
    let over = |v: &[f64], t: f64| v.iter().filter(|x| **x > t).count();
    println!("[{name}] stations {} | axis-terrain p50 {:+.3} p90 {:+.3} max {:+.3} (p90 w/o lift {:+.3})",
        st.len(), pct(&mut axis.clone(), 0.5), pct(&mut axis.clone(), 0.9), pct(&mut axis.clone(), 1.0), pct(&mut no_lift, 0.9));
    println!(
        "   edge-terrain p50 {:+.3} p90 {:+.3} max {:+.3} | >0.25m {:.1}% >0.6m {:.1}%",
        pct(&mut edge.clone(), 0.5),
        pct(&mut edge.clone(), 0.9),
        pct(&mut edge.clone(), 1.0),
        100.0 * over(&edge, 0.25) as f64 / edge.len() as f64,
        100.0 * over(&edge, 0.6) as f64 / edge.len() as f64
    );
    println!(
        "   |ribbon edge - axis| p50 {:.3} p90 {:.3} | |terrain edge - axis| p50 {:.3} p90 {:.3}",
        pct(&mut rib.clone(), 0.5),
        pct(&mut rib, 0.9),
        pct(&mut ter.clone(), 0.5),
        pct(&mut ter, 0.9)
    );
    let mut worst: Vec<&&Station> = st.iter().collect();
    worst.sort_by(|a, b| {
        b.d_edge[0]
            .max(b.d_edge[1])
            .total_cmp(&a.d_edge[0].max(a.d_edge[1]))
    });
    for s in worst.iter().take(5) {
        println!("   worst at ({:.1},{:.1}) axis {:+.3} edges {:+.3}/{:+.3} rib {:+.3}/{:+.3} terr {:+.3}/{:+.3}",
            s.pos[0], s.pos[1], s.d_axis, s.d_edge[0], s.d_edge[1], s.rib[0], s.rib[1], s.ter[0], s.ter[1]);
    }
}

#[test]
#[ignore]
fn road_clearance() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache");
    let id = std::env::var("MEASURE_MAP").unwrap_or_else(|_| "casa".into());
    let index = open_map(&root, &id).expect("open map");
    let tm = index.meta.tile_size_m;
    let mut geo: HashMap<(i32, i32), TileGeometry> = HashMap::new();
    for &(x, y) in index.coords() {
        geo.insert((x, y), index.tile_geometry(x, y).expect("tile_geometry"));
    }
    let spawn = find_spawn(&index, 500.0)
        .expect("spawn")
        .map(|s| [s.x, s.y])
        .unwrap_or([0.0, 0.0]);
    let mut all: Vec<Station> = Vec::new();
    let mut missing = 0usize;
    for g in geo.values() {
        let v = &g.roads.vertices;
        // Vertices come in (left, center, right) triples per station; y is -z in the world.
        let at = |k: usize| -> ([f64; 2], f64) {
            let q = v[k];
            ([q[0] as f64, -(q[2] as f64)], q[1] as f64)
        };
        for s in 0..v.len() / 3 {
            let (pl, hl) = at(3 * s);
            let (pc, hc) = at(3 * s + 1);
            let (pr, hr) = at(3 * s + 2);
            let (Some(tl), Some(tc), Some(tr)) = (
                terrain_at(&geo, tm, pl),
                terrain_at(&geo, tm, pc),
                terrain_at(&geo, tm, pr),
            ) else {
                missing += 1;
                continue;
            };
            all.push(Station {
                pos: pc,
                d_axis: hc - tc,
                d_edge: [hl - tl, hr - tr],
                rib: [hl - hc, hr - hc],
                ter: [tl - tc, tr - tc],
            });
        }
    }
    println!(
        "map {id}: {} road stations, {missing} without terrain",
        all.len()
    );
    let dist = |s: &Station, c: [f64; 2]| (s.pos[0] - c[0]).hypot(s.pos[1] - c[1]);
    for (name, c, r) in [
        ("casa 300m", [0.0, 0.0], 300.0),
        ("casa 500m", [0.0, 0.0], 500.0),
        ("spawn 150m", spawn, 150.0),
    ] {
        let sel: Vec<&Station> = all.iter().filter(|s| dist(s, c) <= r).collect();
        summarize(name, &sel);
    }
}

/// P1 breakdown on real tiles: opening the neighborhood (file reads) against `Map::tile_geometry`
/// with it already open. Run: `cargo test --release -p driving_the_earth_ext tile_perf_real -- --ignored --nocapture`.
#[test]
#[ignore]
fn tile_perf_real() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache");
    let id = std::env::var("MEASURE_MAP").unwrap_or_else(|_| "casa-10km".into());
    let index = open_map(&root, &id).expect("open map");
    let reps = 30;
    for (x, y) in [(-1, -2), (0, 0), (-2, -1)] {
        let (mut open, mut map_ms) = (Vec::new(), Vec::new());
        let mut tris = 0;
        for _ in 0..reps {
            let t0 = Instant::now();
            let map = index.neighborhood(x, y).expect("neighborhood");
            open.push(t0.elapsed().as_secs_f64() * 1e3);
            let center = map
                .tiles
                .iter()
                .find(|t| (t.x, t.y) == (x, y))
                .expect("center");
            let t1 = Instant::now();
            let g = map.tile_geometry(center);
            map_ms.push(t1.elapsed().as_secs_f64() * 1e3);
            tris = g.terrain.indices.len() / 3
                + g.roads.indices.len() / 3
                + g.buildings.indices.len() / 3;
        }
        println!(
            "tile ({x},{y}) tri {tris} | open p50 {:.2} p90 {:.2} | Map::tile_geometry p50 {:.2} p90 {:.2}",
            pct(&mut open.clone(), 0.5),
            pct(&mut open, 0.9),
            pct(&mut map_ms.clone(), 0.5),
            pct(&mut map_ms, 0.9)
        );
    }
}

/// Drawn terrain at each side of the LOD bands (item 54): triangles per tile and per 1 km² (16
/// tiles), `tile_geometry` time, and the step of the recorded collision heightfield across the
/// shared edge of neighboring tiles. Run: `cargo test --release -p driving_the_earth_ext lod_cells -- --ignored --nocapture`.
#[test]
#[ignore]
fn lod_cells_casa() {
    use tileformat::units::{GROUND_CELL_M, TERRAIN_CELL_M, TERRAIN_FINE_CELL_M, TERRAIN_FINE_N};
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache");
    let id = std::env::var("MEASURE_MAP").unwrap_or_else(|_| "casa".into());
    let index = open_map(&root, &id).expect("open map");
    let tile_m = index.meta.tile_size_m;
    let coords = index.coords().to_vec();
    let n = TERRAIN_FINE_N;

    // Collision step across tile edges, from the recorded heightfield (independent of the cell).
    let mut collision: HashMap<(i32, i32), Vec<f32>> = HashMap::new();
    for &(x, y) in &coords {
        let g = index
            .tile_geometry_at(x, y, TERRAIN_FINE_CELL_M)
            .expect("tile_geometry");
        collision.insert((x, y), g.collision);
    }
    let mut step = 0.0f64;
    for &(x, y) in &coords {
        let a = &collision[&(x, y)];
        if let Some(b) = collision.get(&(x + 1, y)) {
            for i in 0..n {
                step = step.max((a[i * n + n - 1] - b[i * n]).abs() as f64);
            }
        }
        if let Some(b) = collision.get(&(x, y + 1)) {
            for j in 0..n {
                step = step.max((a[(n - 1) * n + j] - b[j]).abs() as f64);
            }
        }
    }
    println!(
        "map {id}: {} tiles, tile {tile_m} m, collision edge step max {step:.4} m",
        coords.len()
    );

    // Geometry time only: the neighborhood is read outside the timed call, as in `tile_perf_real`.
    let cells = [TERRAIN_FINE_CELL_M, GROUND_CELL_M, TERRAIN_CELL_M];
    let mut per_cell: Vec<(Vec<f64>, Vec<f64>)> = vec![(Vec::new(), Vec::new()); cells.len()];
    for &(x, y) in &coords {
        let map = index.neighborhood(x, y).expect("neighborhood");
        let center = map
            .tiles
            .iter()
            .find(|t| (t.x, t.y) == (x, y))
            .expect("center");
        for (k, &cell) in cells.iter().enumerate() {
            let t0 = Instant::now();
            let g = map.tile_geometry_at(center, cell);
            per_cell[k].1.push(t0.elapsed().as_secs_f64() * 1e3);
            per_cell[k].0.push((g.terrain.indices.len() / 3) as f64);
        }
    }
    for (cell, (tris, mut ms)) in cells.into_iter().zip(per_cell) {
        let mean = tris.iter().sum::<f64>() / tris.len() as f64;
        println!(
            "cell {cell:>4} m: terrain tri per tile p50 {:.0} p90 {:.0} | per 1 km² {:.0} | tile_geometry ms p50 {:.2} p90 {:.2}",
            pct(&mut tris.clone(), 0.5),
            pct(&mut tris.clone(), 0.9),
            mean * 16.0,
            pct(&mut ms.clone(), 0.5),
            pct(&mut ms, 0.9),
        );
    }
}
