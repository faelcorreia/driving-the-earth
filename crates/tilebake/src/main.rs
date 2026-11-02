//! CLI do bake. Endereço vem de `TILEBAKE_ENDERECO` (não versionado).
use std::path::Path;

fn main() {
    let endereco = match std::env::var("TILEBAKE_ENDERECO") {
        Ok(e) if !e.trim().is_empty() => e,
        _ => {
            eprintln!("Defina TILEBAKE_ENDERECO com o endereço da casa (ver CLAUDE.md).");
            std::process::exit(2);
        }
    };
    let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache");
    match tilebake::baixar_osm(&cache, "casa", "Casa", &endereco) {
        Ok(r) => {
            println!(
                "meta: id={} raio={}m centro=({:.6}, {:.6})",
                r.meta.id, r.meta.radius_m, r.meta.center_lat, r.meta.center_lon
            );
            println!("geocodificação: {}", if r.geocodificacao_em_cache { "cache" } else { "Nominatim" });
            println!("osm: {}", if r.osm_em_cache { "cache" } else { "Overpass" });
            println!("vias: {}", r.vias);
            println!("prédios: {}", r.predios);
        }
        Err(e) => {
            eprintln!("erro: {e}");
            std::process::exit(1);
        }
    }
}
