//! Tile geometry layer: terrain grid, road ribbons and junctions of one tile, as plain meshes.
//! Pure Rust, with unit tests: no Godot types here (no `godot` import anywhere in this module).
//! May import: `tileformat` (data contract), `tilebake` (shared profile math) and `style` (the
//! active theme palette). Nothing else in `gdext`. `world` and `render` use it; it must not use
//! them.

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

use tileformat::units::{GROUND_CELL_M, TERRAIN_CELL_M, TERRAIN_FINE_CELL_M, TERRAIN_FINE_N};
use tileformat::Tile;

use building::building_mesh;
use field::{Field, Surface};
use ground::terrain_colors;
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

/// Terrain, ribbons and buildings of one tile, drawn terrain at `TERRAIN_FINE_CELL_M` (tests only;
/// the game uses `tile_geometry_at`).
#[cfg(test)]
pub fn tile_geometry_with(
    tile: &Tile,
    tile_m: f64,
    ground: &dyn Fn(f64, f64) -> f32,
    around: &[&Tile],
) -> TileGeometry {
    tile_geometry_at(tile, tile_m, ground, around, TERRAIN_FINE_CELL_M)
}

/// Side of the drawn terrain grid for a tile `dist_m` meters from the car, given the side it was
/// built with (`None` when it has no geometry yet). Within 250 m: 2,5 m; to 500 m: 5 m; beyond:
/// 10 m. A tile keeps its side until its distance is `MESH_CELL_HYSTERESIS_M` past the edge, so a
/// car at the edge does not rebuild it on every step.
pub fn mesh_cell_for(dist_m: f64, built: Option<f64>) -> f64 {
    let cells = [TERRAIN_FINE_CELL_M, GROUND_CELL_M, TERRAIN_CELL_M];
    let edges = [MESH_NEAR_M, MESH_MID_M];
    let band_of = |d: f64| edges.iter().filter(|&&e| d >= e).count();
    let Some(cur) = built.and_then(|c| cells.iter().position(|&x| x == c)) else {
        return cells[band_of(dist_m)];
    };
    let raw = band_of(dist_m);
    let band = if raw > cur && dist_m >= edges[cur] + MESH_CELL_HYSTERESIS_M {
        raw
    } else if raw < cur && dist_m < edges[cur - 1] - MESH_CELL_HYSTERESIS_M {
        raw
    } else {
        cur
    };
    cells[band]
}

/// Edges of the drawn terrain bands, in meters from the car (see `mesh_cell_for`).
pub const MESH_NEAR_M: f64 = 250.0;
pub const MESH_MID_M: f64 = 500.0;
/// Margin past a band edge before a tile is rebuilt at its new side.
pub const MESH_CELL_HYSTERESIS_M: f64 = 25.0;

/// Terrain, ribbons and buildings of one tile. `ground(x, y)` is the 10 m grid at any map point;
/// `around` are the neighboring tiles, whose roads count near the borders of this one. The drawn
/// terrain has a vertex every `mesh_cell_m` (a multiple of `TERRAIN_FINE_CELL_M`), sampled from
/// the same field as the ribbons, so they lie on it. The collision heightfield is the recorded
/// 2,5 m grid with the road profiles applied (`collision_heights`), so the ribbons lie on it too,
/// whatever the drawn side.
pub fn tile_geometry_at(
    tile: &Tile,
    tile_m: f64,
    ground: &dyn Fn(f64, f64) -> f32,
    around: &[&Tile],
    mesh_cell_m: f64,
) -> TileGeometry {
    let cell = mesh_cell_m;
    let n = (tile_m / cell).round() as usize + 1;
    let (x0, y0) = (tile.x as f64 * tile_m, tile.y as f64 * tile_m);
    let origin = [x0 - tile_m, y0 - tile_m];
    let field = Field::new(
        std::iter::once(tile)
            .chain(around.iter().copied())
            .flat_map(|t| t.roads.iter()),
        origin,
    );
    // The collision follows the same profiles on the 2,5 m grid (`collision_heights`). A mesh on that
    // grid already has them in its interior, so the collision reads it; a coarser mesh gets its own
    // surface on the fine grid, from a copy of the field made before the surface takes it.
    let fine = cell == TERRAIN_FINE_CELL_M;
    let fine_field = (!fine).then(|| field.clone());
    let surface = Surface::new(field, tile, tile_m, ground, [x0, y0], n, cell);
    let collision = if fine {
        surface.collision_grid(&|x, y| sample_height(tile, tile_m, x, y))
    } else {
        collision_heights(tile, tile_m, fine_field.filter(|f| !f.is_empty()))
    };
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
    let ends = extend_junction_ends(&tile.roads, around, tile_m);
    for (idx, (road, pts)) in tile.roads.iter().zip(&ends).enumerate() {
        let half = road_width_m(&road.highway) / 2.0;
        let lift = ROAD_LIFT_M + (idx % 8) as f32 * ROAD_ORDER_LIFT_M;
        push_road(&mut roads, &surface, ground, pts, half, lift);
    }

    let (buildings, building_prisms) = building_mesh(tile, ground);
    let lamps = lamps::lamp_posts(tile, ground);
    TileGeometry {
        terrain,
        roads,
        buildings,
        building_prisms,
        lamps,
        collision,
        mesh_cell_m,
    }
}

/// The 2,5 m heightfield of the tile's collision, `TERRAIN_FINE_N` squared, row-major: the recorded
/// grid (`sample_height`, so a tile baked without one falls back to its 10 m grid), with the road
/// profiles applied where they are. A road with a profile lifts or lowers the ground under it, and
/// its ribbon sits on that profile: the collision must follow it, or the ribbon hangs over the
/// ground (a car whose wheels reach only the ground then stops on the ribbon's edge). The ground
/// passed to the surface is the recorded grid itself, so the points away from every road keep it.
/// `field` is the road field of the tile and its neighbors, `None` when there is no road in them.
fn collision_heights(tile: &Tile, tile_m: f64, field: Option<Field>) -> Vec<f32> {
    let n = TERRAIN_FINE_N;
    let (x0, y0) = (tile.x as f64 * tile_m, tile.y as f64 * tile_m);
    let cell = TERRAIN_FINE_CELL_M;
    let recorded = |x: f64, y: f64| sample_height(tile, tile_m, x, y);
    match field {
        Some(field) => Surface::new(field, tile, tile_m, &recorded, [x0, y0], n, cell).heights,
        None => (0..n * n)
            .map(|k| {
                let (i, j) = (k / n, k % n);
                recorded(x0 + j as f64 * cell, y0 + i as f64 * cell)
            })
            .collect(),
    }
}

#[cfg(test)]
mod lod_tests {
    use super::*;

    #[test]
    fn mesh_cell_follows_the_bands() {
        assert_eq!(mesh_cell_for(0.0, None), TERRAIN_FINE_CELL_M);
        assert_eq!(mesh_cell_for(249.0, None), TERRAIN_FINE_CELL_M);
        assert_eq!(mesh_cell_for(250.0, None), GROUND_CELL_M);
        assert_eq!(mesh_cell_for(499.0, None), GROUND_CELL_M);
        assert_eq!(mesh_cell_for(500.0, None), TERRAIN_CELL_M);
        assert_eq!(mesh_cell_for(900.0, None), TERRAIN_CELL_M);
    }

    #[test]
    fn a_tile_keeps_its_side_inside_the_hysteresis_margin() {
        // Built at 2,5 m: leaves for 5 m only past 250 + 25 m.
        assert_eq!(
            mesh_cell_for(260.0, Some(TERRAIN_FINE_CELL_M)),
            TERRAIN_FINE_CELL_M
        );
        assert_eq!(
            mesh_cell_for(280.0, Some(TERRAIN_FINE_CELL_M)),
            GROUND_CELL_M
        );
        // Built at 5 m: returns to 2,5 m only below 250 - 25 m.
        assert_eq!(mesh_cell_for(240.0, Some(GROUND_CELL_M)), GROUND_CELL_M);
        assert_eq!(
            mesh_cell_for(220.0, Some(GROUND_CELL_M)),
            TERRAIN_FINE_CELL_M
        );
        // A jump over a band (car teleported) moves the tile to the band it is in.
        assert_eq!(
            mesh_cell_for(700.0, Some(TERRAIN_FINE_CELL_M)),
            TERRAIN_CELL_M
        );
    }
}
