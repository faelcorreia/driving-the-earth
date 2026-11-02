//! Validação do F3: baixa (ou reaproveita do cache) o DEM do centro de `cache/casa/meta.json`,
//! lê e suaviza, e imprime a altura no centro e o mín/máx das grades dos tiles que
//! cobrem o raio de 1 km. Rodar: `cargo run -p tilebake --example dem_centro`.
use std::fs;
use std::path::Path;

use tilebake::dem::{altura_em, alturas_do_tile, baixar_dem, ler_dem_suavizado, Grade};
use tileformat::MapMeta;

fn main() {
    let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache");
    let meta_txt = fs::read_to_string(cache.join("casa/meta.json")).expect("falta cache/casa/meta.json");
    let meta: MapMeta = serde_json::from_str(&meta_txt).expect("meta.json inválido");

    let (tif, em_cache) = baixar_dem(&cache, &meta.id, meta.center_lat, meta.center_lon).expect("download do DEM");
    println!("dem: {} ({})", tif.display(), if em_cache { "cache" } else { "baixado" });

    let raster = ler_dem_suavizado(&tif).expect("leitura do GeoTIFF");
    println!("raster: {}x{} px, passo {:.6}° ", raster.largura, raster.altura, raster.passo_lon);

    let centro = altura_em(&raster, meta.center_lat, meta.center_lon).expect("centro fora do DEM");
    println!("altura no centro ({:.6}, {:.6}): {:.1} m", meta.center_lat, meta.center_lon, centro);

    let grade = Grade::de_meta(&meta);
    let (mut min, mut max) = (f32::INFINITY, f32::NEG_INFINITY);
    let mut tiles = 0;
    // Tiles de 250 m cobrindo [-1000, 1000] m em x e y (raio de 1 km).
    for x in -4..=3 {
        for y in -4..=3 {
            let g = alturas_do_tile(&raster, &grade, x, y).expect("tile sem dado");
            for v in &g.values {
                min = min.min(*v);
                max = max.max(*v);
            }
            tiles += 1;
        }
    }
    println!("tiles: {tiles} ({}x{} amostras cada)", 26, 26);
    println!("altura mín: {min:.1} m   máx: {max:.1} m   amplitude: {:.1} m", max - min);
}
