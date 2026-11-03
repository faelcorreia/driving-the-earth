//! Tile geometry layer: terrain grid, road ribbons and junctions of one tile, as plain meshes.
//! Pure Rust, with unit tests: no Godot types here (no `godot` import anywhere in this module).
//! May import: `tileformat` (data contract) and `tilebake` (shared profile math). Nothing else in
//! `gdext`. `world` and `render` use it; it must not use them.

mod building;
mod field;
mod ground;
mod junction;
pub mod lamps;
mod mesh;
mod profile_smooth;
mod road;
mod terrain;
#[cfg(test)]
pub(crate) mod tests;
mod vec2;

use tileformat::units::TERRAIN_FINE_CELL_M;
use tileformat::Tile;

use building::building_mesh;
use field::{Field, Surface};
use ground::terrain_colors;
pub use ground::GRASS_RGB;
use junction::extend_junction_ends;
use mesh::{normalize, world};
pub use mesh::{MeshData, TileGeometry};
pub(crate) use road::ROAD_LIFT_M;
use road::{push_road, ROAD_ORDER_LIFT_M};
pub use terrain::sample_height;
pub use tileformat::road_width_m;

/// Builds the terrain and the road ribbons of one tile, with no neighbors (tests only;
/// the game uses `Map::tile_geometry`, which sees the neighbors).
#[cfg(test)]
pub fn tile_geometry(tile: &Tile, tile_m: f64) -> TileGeometry {
    tile_geometry_with(tile, tile_m, &|x, y| sample_height(tile, tile_m, x, y), &[])
}

/// Terrain, ribbons and buildings of one tile. `ground(x, y)` is the 10 m grid at any map point;
/// `around` are the neighboring tiles, whose roads count near the borders of this one. The
/// terrain grid has a vertex every `TERRAIN_FINE_CELL_M` (2,5 m), and the ribbons sample the same
/// field, so they lie on the terrain mesh.
pub fn tile_geometry_with(
    tile: &Tile,
    tile_m: f64,
    ground: &dyn Fn(f64, f64) -> f32,
    around: &[&Tile],
) -> TileGeometry {
    let cell = TERRAIN_FINE_CELL_M;
    let n = (tile_m / cell).round() as usize + 1;
    let (x0, y0) = (tile.x as f64 * tile_m, tile.y as f64 * tile_m);
    let origin = [x0 - tile_m, y0 - tile_m];
    let field = Field::new(
        std::iter::once(tile)
            .chain(around.iter().copied())
            .flat_map(|t| t.roads.iter()),
        origin,
    );
    let surface = Surface::new(field, tile, tile_m, ground, [x0, y0], n, cell);
    let h = |i: usize, j: usize| surface.heights[i * n + j];

    let mut terrain = MeshData::default();
    for i in 0..n {
        for j in 0..n {
            // Normal from central differences (one-sided at the tile edges).
            let (j0, j1) = (j.saturating_sub(1), (j + 1).min(n - 1));
            let (i0, i1) = (i.saturating_sub(1), (i + 1).min(n - 1));
            let dh_dx = (h(i, j1) - h(i, j0)) as f64 / ((j1 - j0) as f64 * cell);
            let dh_dy = (h(i1, j) - h(i0, j)) as f64 / ((i1 - i0) as f64 * cell);
            terrain
                .vertices
                .push(world(x0 + j as f64 * cell, y0 + i as f64 * cell, h(i, j)));
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

    terrain.colors = terrain_colors(tile, n, cell);

    let mut roads = MeshData::default();
    let ends = extend_junction_ends(&tile.roads, tile_m);
    for (idx, (road, pts)) in tile.roads.iter().zip(&ends).enumerate() {
        let half = road_width_m(&road.highway) / 2.0;
        let lift = ROAD_LIFT_M + (idx % 8) as f32 * ROAD_ORDER_LIFT_M;
        push_road(&mut roads, &surface, ground, pts, half, lift);
    }

    let buildings = building_mesh(tile, ground);
    let lamps = lamps::lamp_posts(tile, ground);
    TileGeometry {
        terrain,
        roads,
        buildings,
        lamps,
    }
}
