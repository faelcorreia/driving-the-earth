//! Bake CLI. The address comes from `TILEBAKE_ADDRESS` (not versioned).
use std::path::Path;

fn main() {
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
            println!("osm: {}", if r.osm_cached { "cache" } else { "Overpass" });
            println!("roads: {}", r.roads);
            println!("buildings: {}", r.buildings);
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
