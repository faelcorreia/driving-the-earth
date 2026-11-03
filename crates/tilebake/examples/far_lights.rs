//! Writes `cache/<dir>/far_lights.bin` from the baked tiles (no rebake, no download):
//! `cargo run --release -p tilebake --example far_lights -- casa-10km`.
//! Prints the number of lights, the file size, and the lights in the serra sector (140-210 deg, 3-9 km).
use std::path::Path;

use tileformat::far_lights::read_far_lights;

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "casa-10km".into());
    let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache");
    let started = std::time::Instant::now();
    let s = tilebake::far_lights::bake_far_lights(&cache, &dir).expect("far_lights bake");
    let secs = started.elapsed().as_secs_f64();
    let bin = cache.join(&dir).join("far_lights.bin");
    let bytes = std::fs::metadata(&bin).map(|m| m.len()).unwrap_or(0);
    let lights = read_far_lights(&bin).expect("read back");
    let in_sector = lights
        .iter()
        .filter(|l| {
            let r = (l.x as f64).hypot(l.y as f64);
            let az = (l.x as f64).atan2(l.y as f64).to_degrees().rem_euclid(360.0);
            (3000.0..=9000.0).contains(&r) && (140.0..=210.0).contains(&az)
        })
        .count();
    println!(
        "{dir}: tiles {} buildings {} (generated {}) cells {} lights {} file {} bytes in {:.1} s",
        s.tiles, s.buildings, s.generated, s.cells, lights.len(), bytes, secs
    );
    println!("{dir}: lights in sector 140-210 deg, 3-9 km: {in_sector}");
}
