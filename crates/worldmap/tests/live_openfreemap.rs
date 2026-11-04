//! Live smoke test against OpenFreeMap (needs the internet). Not run by default:
//! `cargo test -p worldmap --test live_openfreemap -- --ignored --nocapture`
use std::time::Instant;

use worldmap::fetch::{load_template, load_tile, UreqTiles};
use worldmap::tiles::{decode, is_major_class, TileId};

#[test]
#[ignore]
fn tilejson_and_one_belo_horizonte_tile() {
    let dir = std::env::temp_dir().join("worldmap-live-smoke");
    let _ = std::fs::remove_dir_all(&dir);
    let sleep = |d: std::time::Duration| std::thread::sleep(d);

    let t0 = Instant::now();
    let template = load_template(&dir, &UreqTiles, &sleep).expect("TileJSON");
    println!(
        "tilejson: {:?} ({:.0} ms)",
        template,
        t0.elapsed().as_secs_f64() * 1e3
    );

    // Belo Horizonte centre (-19.9167, -43.9345) at z14.
    let (ux, uy) = worldmap::projection::to_unit(-19.9167, -43.9345);
    let n = 16384.0;
    let id = TileId {
        z: 14,
        x: (ux * n) as u32,
        y: (uy * n) as u32,
    };
    let t1 = Instant::now();
    let (bytes, source) = load_tile(&dir, &template, id, &UreqTiles, &sleep).expect("tile");
    println!(
        "tile {:?} {:?}: {} bytes in {:.0} ms",
        id,
        source,
        bytes.len(),
        t1.elapsed().as_secs_f64() * 1e3
    );

    let data = decode(id, &bytes).expect("decode");
    println!(
        "roads {}, road names {}, places {}",
        data.roads.len(),
        data.road_names.len(),
        data.places.len()
    );
    let mut classes: std::collections::BTreeMap<&str, usize> = Default::default();
    for r in &data.roads {
        *classes.entry(r.class.as_str()).or_default() += 1;
    }
    println!("road classes: {classes:?}");
    let majors = data
        .roads
        .iter()
        .filter(|r| is_major_class(&r.class))
        .count();
    println!("major roads {majors}");
    let mut places: Vec<&str> = data.places.iter().map(|p| p.name.as_str()).collect();
    places.truncate(15);
    println!("place names (first 15): {places:?}");
    assert!(!data.roads.is_empty());
}
