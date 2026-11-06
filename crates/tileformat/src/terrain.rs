//! Terrain height sampling on the drawn triangles of a tile grid (pure Rust; shared by the game and the
//! bake of the distant lights).
use crate::units::{TERRAIN_FINE_CELL_M, TERRAIN_FINE_N};
use crate::{HeightGrid, Heightfield, Tile};

/// Terrain height at local `(x, y)`, on the same triangles that `tile_geometry` draws
/// (each grid cell is split along the diagonal from its north-east to its south-west corner).
/// Sampling the bilinear surface instead would differ from the drawn mesh on slopes, so
/// anything placed "on" the terrain could end up under it.
/// Points outside the tile extrapolate the plane of the nearest edge cell, which matches the
/// neighbor tile along the shared edge. The grid has `cols` samples per side over `tile_m`
/// meters, edges included.
pub fn sample_height(tile: &Tile, tile_m: f64, x: f64, y: f64) -> f32 {
    sample_local(
        &tile.height,
        tile_m,
        x - tile.x as f64 * tile_m,
        y - tile.y as f64 * tile_m,
    )
}

/// `sample_height` for a grid of one tile, at `(lx, ly)` meters from the tile corner.
pub fn sample_local(grid: &HeightGrid, tile_m: f64, lx: f64, ly: f64) -> f32 {
    let n = grid.cols as usize;
    let cell = tile_m / (n - 1) as f64;
    let u = lx / cell;
    let v = ly / cell;
    let j0 = (u.floor().max(0.0) as usize).min(n - 2);
    let i0 = (v.floor().max(0.0) as usize).min(n - 2);
    let (fu, fv) = (u - j0 as f64, v - i0 as f64);
    let at = |i: usize, j: usize| grid.values[i * n + j] as f64;
    let (a, b, c, d) = (
        at(i0, j0),
        at(i0, j0 + 1),
        at(i0 + 1, j0),
        at(i0 + 1, j0 + 1),
    );
    let h = if fu + fv <= 1.0 {
        // Triangle a, b, c.
        a + (b - a) * fu + (c - a) * fv
    } else {
        // Triangle b, c, d.
        d + (c - d) * (1.0 - fu) + (b - d) * (1.0 - fv)
    };
    h as f32
}

/// The fine heightfield of a tile (`TERRAIN_FINE_N` samples per side, every `TERRAIN_FINE_CELL_M`
/// meters, edges included), read from its final grid with `sample_local`: the same surface the
/// game draws, so neighbor tiles agree on their shared edge. `height` is the tile's final grid
/// (regions already applied).
pub fn fine_heightfield(height: &HeightGrid, tile_m: f64) -> Result<Heightfield, String> {
    let n = TERRAIN_FINE_N;
    let mut values = Vec::with_capacity(n * n);
    for i in 0..n {
        for j in 0..n {
            let lx = j as f64 * TERRAIN_FINE_CELL_M;
            let ly = i as f64 * TERRAIN_FINE_CELL_M;
            values.push(sample_local(height, tile_m, lx, ly));
        }
    }
    Heightfield::from_meters(n as u32, n as u32, &values)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smooth, non-linear terrain in absolute meters (about 900 m high, relief of a few meters).
    fn ground(x: f64, y: f64) -> f32 {
        (900.0 + 0.03 * x + 0.02 * y + 5.0 * (x / 37.0).sin() * (y / 53.0).cos()) as f32
    }

    /// The 10 m grid of tile `(tx, ty)`, sampled from `ground` at absolute coordinates.
    fn tile_grid(tx: i32, ty: i32, tile_m: f64) -> HeightGrid {
        let cell = 10.0;
        let n = (tile_m / cell) as usize + 1;
        let (x0, y0) = (tx as f64 * tile_m, ty as f64 * tile_m);
        let values = (0..n)
            .flat_map(|i| (0..n).map(move |j| (i, j)))
            .map(|(i, j)| ground(x0 + j as f64 * cell, y0 + i as f64 * cell))
            .collect();
        HeightGrid {
            cols: n as u32,
            rows: n as u32,
            values,
        }
    }

    fn absolute_cm(f: &Heightfield, k: usize) -> i64 {
        f.base_cm as i64 + f.cm[k] as i64
    }

    #[test]
    fn fine_grid_is_101_by_101_over_a_250_m_tile() {
        let fine = fine_heightfield(&tile_grid(0, 0, 250.0), 250.0).unwrap();
        assert_eq!((fine.cols, fine.rows), (101, 101));
        assert_eq!(fine.cm.len(), 101 * 101);
    }

    #[test]
    fn fine_samples_read_back_within_1_cm_of_the_surface() {
        let grid = tile_grid(2, -1, 250.0);
        let fine = fine_heightfield(&grid, 250.0).unwrap();
        for i in 0..101 {
            for j in 0..101 {
                let lx = j as f64 * 2.5;
                let ly = i as f64 * 2.5;
                let want = sample_local(&grid, 250.0, lx, ly);
                let got = fine.meters(i * 101 + j);
                assert!((got - want).abs() <= 0.01, "({i},{j}): {got} vs {want}");
            }
        }
    }

    #[test]
    fn shared_edges_of_neighbor_tiles_are_identical() {
        let tile_m = 250.0;
        let a = fine_heightfield(&tile_grid(0, 0, tile_m), tile_m).unwrap();
        let east = fine_heightfield(&tile_grid(1, 0, tile_m), tile_m).unwrap();
        let north = fine_heightfield(&tile_grid(0, 1, tile_m), tile_m).unwrap();
        for i in 0..101 {
            // East edge of A is the west edge of its east neighbor.
            assert_eq!(
                absolute_cm(&a, i * 101 + 100),
                absolute_cm(&east, i * 101),
                "east edge row {i}"
            );
            // North edge of A (last row) is the south edge of the tile above it.
            assert_eq!(
                absolute_cm(&a, 100 * 101 + i),
                absolute_cm(&north, i),
                "north edge column {i}"
            );
        }
    }

    #[test]
    fn sample_height_is_unchanged_by_the_local_refactor() {
        let grid = tile_grid(3, 4, 250.0);
        let tile = Tile {
            x: 3,
            y: 4,
            height: grid.clone(),
            roads: vec![],
            buildings: vec![],
            ground: vec![],
            fine_height: None,
        };
        let (x, y) = (3.0 * 250.0 + 61.3, 4.0 * 250.0 + 17.9);
        let (lx, ly) = (x - 3.0 * 250.0, y - 4.0 * 250.0);
        assert_eq!(
            sample_height(&tile, 250.0, x, y),
            sample_local(&grid, 250.0, lx, ly)
        );
    }
}
