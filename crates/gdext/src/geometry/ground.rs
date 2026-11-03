//! Terrain vertex colors from the ground cover of a tile: grass, cement, sidewalk, park and water.
//! Pure Rust (no Godot types). Imports `tileformat` data only.
//!
//! Each terrain vertex takes the mean color of the 2x2 ground cells (5 m) under its 10 m window,
//! so the color changes smoothly from one vertex to the next. No outline: the sharp edge between
//! classes waits for the constrained triangulation.
use tileformat::units::{GROUND_CELL_M, GROUND_N};
use tileformat::{Tile, GROUND_CONCRETE, GROUND_PARK, GROUND_SIDEWALK, GROUND_WATER};

/// Grass and dirt, sRGB (the terrain color before the ground cover).
pub const GRASS_RGB: [f32; 3] = [0.42, 0.55, 0.36];
const CEMENT_RGB: [f32; 3] = [0.50, 0.50, 0.51];
const SIDEWALK_RGB: [f32; 3] = [0.62, 0.62, 0.62];
const PARK_RGB: [f32; 3] = [0.30, 0.45, 0.28];
const WATER_RGB: [f32; 3] = [0.25, 0.38, 0.48];

fn class_rgb(class: u8) -> [f32; 3] {
    match class {
        GROUND_CONCRETE => CEMENT_RGB,
        GROUND_SIDEWALK => SIDEWALK_RGB,
        GROUND_PARK => PARK_RGB,
        GROUND_WATER => WATER_RGB,
        _ => GRASS_RGB,
    }
}

/// One RGBA color per terrain vertex of an `n` x `n` grid with side `cell_m` (row-major, `i` is y,
/// `j` is x), or nothing when the tile has no ground cover (tiles baked before it existed).
pub fn terrain_colors(tile: &Tile, n: usize, cell_m: f64) -> Vec<[f32; 4]> {
    if tile.ground.len() != GROUND_N * GROUND_N {
        return Vec::new();
    }
    let last = GROUND_N - 1;
    // Cells of the 10 m window centered on a vertex: the two cells on each side of it.
    let cells = |v: usize| {
        let c = (v as f64 * cell_m / GROUND_CELL_M) as usize;
        [c.saturating_sub(1), c.min(last)]
    };
    let mut colors = Vec::with_capacity(n * n);
    for i in 0..n {
        let rows = cells(i);
        for j in 0..n {
            let cols = cells(j);
            let mut sum = [0.0f32; 3];
            for &gy in &rows {
                for &gx in &cols {
                    let c = class_rgb(tile.ground[gy * GROUND_N + gx]);
                    for k in 0..3 {
                        sum[k] += c[k];
                    }
                }
            }
            colors.push([sum[0] / 4.0, sum[1] / 4.0, sum[2] / 4.0, 1.0]);
        }
    }
    colors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::tests::{grid, tile};
    use tileformat::GROUND_GRASS;

    #[test]
    fn no_ground_means_no_colors() {
        let t = tile(0, 0, grid(26, |_, _| 0.0), Vec::new());
        assert!(terrain_colors(&t, 26, 10.0).is_empty());
    }

    #[test]
    fn a_cement_cell_tints_only_the_vertices_around_it() {
        let mut t = tile(0, 0, grid(26, |_, _| 0.0), Vec::new());
        t.ground = vec![GROUND_GRASS; GROUND_N * GROUND_N];
        // Cell (gx 20, gy 20) covers x and y in [100, 105): the vertices at 100 and 110 touch it.
        t.ground[20 * GROUND_N + 20] = GROUND_CONCRETE;
        let c = terrain_colors(&t, 26, 10.0);
        let at = |i: usize, j: usize| c[i * 26 + j];
        assert_eq!(at(0, 0), [GRASS_RGB[0], GRASS_RGB[1], GRASS_RGB[2], 1.0]);
        // Vertex (10, 10) m: its window holds the cell (20, 20); the mean is 1/4 cement.
        let mix = |g: f32, c: f32| (3.0 * g + c) / 4.0;
        assert!((at(10, 10)[0] - mix(GRASS_RGB[0], CEMENT_RGB[0])).abs() < 1e-6);
        assert!((at(20, 20)[0] - GRASS_RGB[0]).abs() < 1e-6);
    }

}
