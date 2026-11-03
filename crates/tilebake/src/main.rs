//! Bake CLI. The address comes from `TILEBAKE_ADDRESS` (not versioned).
use std::path::Path;

fn main() {
    // `tilebake region <source-dir>`: builds the regional package described by <source-dir>/region.json
    // (contour shapefiles next to it) into cache/regions/<id>/ and adds the id to cache/regions/index.json.
    if std::env::args().nth(1).as_deref() == Some("region") {
        let Some(source) = std::env::args_os().nth(2) else {
            eprintln!("usage: tilebake region <source-dir>   (source-dir holds region.json and the .shp/.dbf files)");
            std::process::exit(2);
        };
        let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache");
        let base = tilebake::region::regions_dir(&cache);
        match tilebake::region::build_from_shapefiles(Path::new(&source), &base) {
            Ok(r) => println!(
                "package {} written to {} ({} polygon vertices); index updated",
                r.manifest.id,
                base.join(&r.manifest.id).display(),
                r.manifest.polygon_lonlat.len()
            ),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    let address = match std::env::var("TILEBAKE_ADDRESS") {
        Ok(e) if !e.trim().is_empty() => e,
        _ => {
            eprintln!("Set TILEBAKE_ADDRESS to the home address (see CLAUDE.md).");
            std::process::exit(2);
        }
    };
    let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache");
    match tilebake::download_osm(&cache, "casa", "Casa", &address) {
        Ok(r) => {
            println!(
                "meta: id={} radius={}m center=({:.6}, {:.6})",
                r.meta.id, r.meta.radius_m, r.meta.center_lat, r.meta.center_lon
            );
            println!("geocoding: {}", if r.geocoding_cached { "cache" } else { "Nominatim" });
            println!(
                "osm: {} blocks, {} downloaded, {:.1} MB on disk",
                r.blocks,
                r.blocks_downloaded,
                r.osm_bytes as f64 / 1e6
            );
            println!("roads: {}", r.roads);
            println!("buildings: {}", r.buildings);
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
    match tilebake::tiles::bake_tiles(&cache, "casa") {
        Ok(t) => {
            println!("tiles: {} written in {:.1} s", t.tiles, t.elapsed.as_secs_f64());
            println!("tiles on disk: {:.1} MB", t.tiles_bytes as f64 / 1e6);
            println!("road pieces: {}", t.road_pieces);
            println!("building pieces: {}", t.building_pieces);
            println!("generated buildings: {}", t.generated_buildings);
            println!("meta.complete: true");
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
