//! Rebakes the tiles of one cache directory from its raw data (`cache/<dir>/raw`), without
//! downloading OSM: `cargo run --release -p tilebake --example bake_cache -- casa-10km`.
use std::path::Path;

fn main() {
    let dir = std::env::args().nth(1).expect("usage: bake_cache <cache-dir-name>");
    let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache");
    let started = std::time::Instant::now();
    let s = tilebake::tiles::bake_tiles(&cache, &dir).expect("bake");
    println!(
        "{dir}: tiles {} road pieces {} building pieces {} bytes {} in {:.1} s",
        s.tiles,
        s.road_pieces,
        s.building_pieces,
        s.tiles_bytes,
        started.elapsed().as_secs_f64()
    );
}
