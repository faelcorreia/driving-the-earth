//! F3 validation: downloads (or reuses the cache of) the DEM at the center of `cache/casa/meta.json`,
//! reads and smooths it, and prints the height at the center and the min/max of the grids of the tiles
//! covering the 1 km radius. Run: `cargo run -p tilebake --example center_dem`.
use std::fs;
use std::path::Path;

use tilebake::dem::{download_dem, height_at, read_smoothed_dem, tile_heights, MapGrid};
use tileformat::MapMeta;

fn main() {
    let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache");
    let meta_txt =
        fs::read_to_string(cache.join("casa/meta.json")).expect("missing cache/casa/meta.json");
    let meta: MapMeta = serde_json::from_str(&meta_txt).expect("invalid meta.json");

    let (tif, from_cache) =
        download_dem(&cache, &meta.id, meta.center_lat, meta.center_lon).expect("DEM download");
    println!(
        "dem: {} ({})",
        tif.display(),
        if from_cache { "cache" } else { "downloaded" }
    );

    let raster = read_smoothed_dem(&tif).expect("GeoTIFF read");
    println!(
        "raster: {}x{} px, step {:.6}° ",
        raster.width, raster.height, raster.step_lon
    );

    let center = height_at(&raster, meta.center_lat, meta.center_lon).expect("center outside DEM");
    println!(
        "height at center ({:.6}, {:.6}): {:.1} m",
        meta.center_lat, meta.center_lon, center
    );

    let grid = MapGrid::from_meta(&meta);
    let (mut min, mut max) = (f32::INFINITY, f32::NEG_INFINITY);
    let mut tiles = 0;
    // 250 m tiles covering [-1000, 1000] m in x and y (1 km radius).
    for x in -4..=3 {
        for y in -4..=3 {
            let g = tile_heights(&raster, &grid, x, y).expect("tile has no data");
            for v in &g.values {
                min = min.min(*v);
                max = max.max(*v);
            }
            tiles += 1;
        }
    }
    println!("tiles: {tiles} ({}x{} samples each)", 26, 26);
    println!(
        "min height: {min:.1} m   max: {max:.1} m   amplitude: {:.1} m",
        max - min
    );
}
