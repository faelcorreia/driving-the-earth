//! Terrain height sampling on the drawn triangles of a tile grid (pure Rust; shared by the game and the
//! bake of the distant lights).
use crate::Tile;

/// Terrain height at local `(x, y)`, on the same triangles that `tile_geometry` draws
/// (each grid cell is split along the diagonal from its north-east to its south-west corner).
/// Sampling the bilinear surface instead would differ from the drawn mesh on slopes, so
/// anything placed "on" the terrain could end up under it.
/// Points outside the tile extrapolate the plane of the nearest edge cell, which matches the
/// neighbor tile along the shared edge. The grid has `cols` samples per side over `tile_m`
/// meters, edges included.
pub fn sample_height(tile: &Tile, tile_m: f64, x: f64, y: f64) -> f32 {
    let grid = &tile.height;
    let n = grid.cols as usize;
    let cell = tile_m / (n - 1) as f64;
    let u = (x - tile.x as f64 * tile_m) / cell;
    let v = (y - tile.y as f64 * tile_m) / cell;
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
