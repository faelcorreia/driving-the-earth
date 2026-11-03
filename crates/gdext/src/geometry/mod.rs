//! Tile geometry layer: terrain grid, road ribbons and junctions of one tile, as plain meshes.
//! Pure Rust, with unit tests: no Godot types here (no `godot` import anywhere in this module).
//! May import: `tileformat` (data contract) and `tilebake` (shared profile math). Nothing else in
//! `gdext`. `world` and `render` use it; it must not use them.

mod building;
mod ground;
mod junction;
mod lamps;
mod mesh;
mod road;
mod terrain;
mod vec2;
#[cfg(test)]
pub(crate) mod tests;

use tileformat::Tile;

use building::building_mesh;
use ground::terrain_colors;
use junction::extend_junction_ends;
pub(crate) use road::ROAD_LIFT_M;
use road::{lower_under_roads, push_road, ROAD_ORDER_LIFT_M, ROAD_STATIONS_PER_CELL};
use mesh::{normalize, world};
pub use ground::GRASS_RGB;
pub use mesh::{MeshData, TileGeometry};
pub use terrain::sample_height;
pub use tileformat::road_width_m;

/// Builds the terrain grid and the road ribbons of one tile, with no neighbors (tests only;
/// the game uses `Map::tile_geometry`, which sees the neighbors).
#[cfg(test)]
pub fn tile_geometry(tile: &Tile, tile_m: f64) -> TileGeometry {
    tile_geometry_with(tile, tile_m, &|x, y| sample_height(tile, tile_m, x, y))
}

/// `tile_geometry` with the terrain height under the roads given by `ground(x, y)`.
pub fn tile_geometry_with(tile: &Tile, tile_m: f64, ground: &dyn Fn(f64, f64) -> f32) -> TileGeometry {
    let n = tile.height.cols as usize;
    let cell = tile_m / (n - 1) as f64;
    let (x0, y0) = (tile.x as f64 * tile_m, tile.y as f64 * tile_m);
    let ends = extend_junction_ends(&tile.roads, tile_m);
    let heights = lower_under_roads(&tile.height.values, n, [x0, y0], cell, &tile.roads, &ends);
    let h = |i: usize, j: usize| heights[i * n + j];

    let mut terrain = MeshData::default();
    for i in 0..n {
        for j in 0..n {
            // Normal from central differences (one-sided at the tile edges).
            let (j0, j1) = (j.saturating_sub(1), (j + 1).min(n - 1));
            let (i0, i1) = (i.saturating_sub(1), (i + 1).min(n - 1));
            let dh_dx = (h(i, j1) - h(i, j0)) as f64 / ((j1 - j0) as f64 * cell);
            let dh_dy = (h(i1, j) - h(i0, j)) as f64 / ((i1 - i0) as f64 * cell);
            terrain.vertices.push(world(
                x0 + j as f64 * cell,
                y0 + i as f64 * cell,
                h(i, j),
            ));
            terrain.normals.push(normalize([-dh_dx, 1.0, dh_dy]));
        }
    }
    for i in 0..n - 1 {
        for j in 0..n - 1 {
            let a = (i * n + j) as u32;
            let (b, c) = (a + 1, a + n as u32);
            let d = c + 1;
            // Godot front faces are clockwise seen from above, so wind them that way.
            terrain.indices.extend([a, c, b, b, c, d]);
        }
    }

    terrain.colors = terrain_colors(tile, n);

    let mut roads = MeshData::default();
    // Stations every quarter of a grid cell: the ribbon follows the terrain between nodes.
    let step = cell / ROAD_STATIONS_PER_CELL;
    for (idx, road) in tile.roads.iter().enumerate() {
        let half = road_width_m(&road.highway) / 2.0;
        let lift = ROAD_LIFT_M + (idx % 8) as f32 * ROAD_ORDER_LIFT_M;
        let (pts, zs) = &ends[idx];
        push_road(&mut roads, ground, pts, zs, half, step, lift);
    }

    let buildings = building_mesh(tile, ground);
    TileGeometry { terrain, roads, buildings }
}
