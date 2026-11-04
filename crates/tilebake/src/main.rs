//! Bake CLI. The address comes from `TILEBAKE_ADDRESS` (not versioned).
use std::path::Path;
use std::sync::atomic::AtomicBool;

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
    if std::env::args().nth(1).as_deref() == Some("at") {
        run_at();
        return;
    }
    let address = match std::env::var("TILEBAKE_ADDRESS") {
        Ok(e) if !e.trim().is_empty() => e,
        _ => {
            eprintln!(
                "Set TILEBAKE_ADDRESS to the start address, or bake by coordinates with `tilebake at <lat> <lon> [id]`."
            );
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
            println!(
                "geocoding: {}",
                if r.geocoding_cached {
                    "cache"
                } else {
                    "Nominatim"
                }
            );
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
            println!(
                "tiles: {} written in {:.1} s",
                t.tiles,
                t.elapsed.as_secs_f64()
            );
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

/// `tilebake at <lat> <lon> [id]`: bakes the whole map centered on a point, in degrees, with no
/// address. The radius comes from `TILEBAKE_RADIUS_M` (default 10000 m). The id defaults to one
/// built from the point. Writes to `cache/<id>/`; finished steps are skipped.
fn run_at() {
    let usage = "usage: tilebake at <lat> <lon> [id]   (degrees; radius from TILEBAKE_RADIUS_M)";
    let (Some(lat_arg), Some(lon_arg)) = (std::env::args().nth(2), std::env::args().nth(3)) else {
        eprintln!("{usage}");
        std::process::exit(2);
    };
    let (Ok(lat), Ok(lon)) = (lat_arg.trim().parse::<f64>(), lon_arg.trim().parse::<f64>()) else {
        eprintln!("error: lat and lon must be numbers\n{usage}");
        std::process::exit(2);
    };
    let id = std::env::args()
        .nth(4)
        .unwrap_or_else(|| tilebake::id_for_coords(lat, lon));
    let radius = match tilebake::radius_from_env() {
        Ok(r) => r.unwrap_or(tilebake::DEFAULT_RADIUS_M),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache");
    let cancel = AtomicBool::new(false);
    let mut progress = |p: tilebake::Progress| {
        eprintln!("progress: {} {}/{}", p.stage, p.done, p.total);
    };
    match tilebake::bake_region(&cache, &id, &id, lat, lon, radius, &cancel, &mut progress) {
        Ok(r) => {
            println!(
                "meta: id={} radius={}m center=({:.6}, {:.6})",
                r.osm.meta.id, r.osm.meta.radius_m, r.osm.meta.center_lat, r.osm.meta.center_lon
            );
            println!(
                "osm: {} blocks, {} downloaded, {:.1} MB on disk",
                r.osm.blocks,
                r.osm.blocks_downloaded,
                r.osm.osm_bytes as f64 / 1e6
            );
            println!(
                "areas: {} blocks, {} downloaded",
                r.areas.blocks, r.areas.blocks_downloaded
            );
            println!(
                "tiles: {} written in {:.1} s",
                r.tiles.tiles,
                r.tiles.elapsed.as_secs_f64()
            );
            println!("far lights: {} points", r.far_lights.points);
            println!("meta.complete: true");
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
