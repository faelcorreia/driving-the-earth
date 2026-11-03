//! Units of the world: every size is defined in METERS, with an exact integer relation to the
//! unit that contains it (see the asserts below). Nothing in the bake or in the game uses a loose
//! magic number for a distance, height or width: it uses one of these names.
//!
//! Hierarchy (decimal, in meters): tile 250 m = 25 terrain cells of 10 m; ground cover grid
//! 5 m = 50 cells per tile (`GROUND_N`). Built geometry uses the module `MODULE_M` = 0.5 m
//! (lot widths, depths and setbacks are multiples of it); a floor is `FLOOR_HEIGHT_M` = 3.0 m;
//! a sidewalk is `SIDEWALK_M` = 2.5 m. Natural geometry (terrain, curved roads, roofs, trees) is
//! continuous and is not quantized; only generators of built geometry snap to `MODULE_M`.
//! Coordinates: the world is f64 in meters from the origin; tile-local values are f32 in meters
//! from the tile corner.

/// Side of a grid tile, in meters.
pub const TILE_M: f64 = 250.0;
/// Side of a terrain (DEM) cell, in meters.
pub const TERRAIN_CELL_M: f64 = 10.0;
/// Side of a ground-cover cell (`Tile::ground`), in meters.
pub const GROUND_CELL_M: f64 = 5.0;
/// Cells per side of the ground-cover grid of one tile.
pub const GROUND_N: usize = (TILE_M / GROUND_CELL_M) as usize;
/// Smallest quantum of built geometry (lot widths, depths and setbacks), in meters.
pub const MODULE_M: f64 = 0.5;
/// Height of one floor of a building, in meters.
pub const FLOOR_HEIGHT_M: f64 = 3.0;
/// Width of a sidewalk along the urban roads, in meters.
pub const SIDEWALK_M: f64 = 2.5;

const _: () = assert!(TILE_M % TERRAIN_CELL_M == 0.0, "a tile holds whole terrain cells");
const _: () = assert!(TILE_M % GROUND_CELL_M == 0.0, "a tile holds whole ground cells");
const _: () = assert!(TERRAIN_CELL_M % GROUND_CELL_M == 0.0, "a terrain cell holds whole ground cells");
const _: () = assert!(GROUND_CELL_M % MODULE_M == 0.0, "ground cells are whole modules");
const _: () = assert!(FLOOR_HEIGHT_M % MODULE_M == 0.0, "a floor is a whole number of modules");
const _: () = assert!(SIDEWALK_M % MODULE_M == 0.0, "a sidewalk is a whole number of modules");
const _: () = assert!(GROUND_N as f64 * GROUND_CELL_M == TILE_M, "the ground grid covers the tile");
