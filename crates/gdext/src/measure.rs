//! F10a baseline (run by hand, not in `cargo test`): triangles per tile and `tile_geometry` time.
//! Run: `cargo test --release -p driving_the_earth_ext measure -- --ignored --nocapture`.
use std::path::Path;
use std::time::Instant;

use crate::world::open_map;

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
    println!("road tri p50 {:.0} p90 {:.0}", pct(&mut road.clone(), 0.5), pct(&mut road.clone(), 0.9));
    println!("terrain tri p50 {:.0} p90 {:.0}", pct(&mut terr.clone(), 0.5), pct(&mut terr.clone(), 0.9));
    println!("building tri p50 {:.0} p90 {:.0}", pct(&mut bld.clone(), 0.5), pct(&mut bld.clone(), 0.9));
    println!("total tri p50 {:.0} p90 {:.0}", pct(&mut all.clone(), 0.5), pct(&mut all, 0.9));
    println!("lamps per tile p50 {:.0} p90 {:.0} (total {})", pct(&mut lamps.clone(), 0.5), pct(&mut lamps.clone(), 0.9), lamps.iter().sum::<f64>());
    println!("tile_geometry ms p50 {:.2} p90 {:.2}", pct(&mut ms.clone(), 0.5), pct(&mut ms, 0.9));
}
