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

/// Lot rules of the generated buildings (B0). Distances in meters; the drawn values are snapped
/// to `MODULE_M`, so every lot edge is a whole number of modules.
pub mod lots {
    use super::MODULE_M;

    /// Lot width range (along the street).
    pub const LOT_W_MIN_M: f64 = 8.0;
    pub const LOT_W_MAX_M: f64 = 20.0;
    /// Lot depth range (away from the street).
    pub const LOT_D_MIN_M: f64 = 25.0;
    pub const LOT_D_MAX_M: f64 = 35.0;
    /// Front setback range (0 to 2 m) and side/back setback range (0 to 1 m).
    pub const FRONT_SETBACK_MAX_M: f64 = 2.0;
    pub const SIDE_SETBACK_MAX_M: f64 = 1.0;
    /// Share of free lots left empty, in per mille (about 5%).
    pub const EMPTY_LOT_PERMILLE: u64 = 50;
    /// Margin that a lot keeps from a real footprint.
    pub const FOOTPRINT_MARGIN_M: f64 = 1.0;
    /// Service roads shorter than this do not split blocks; longer ones count as access roads.
    pub const ACCESS_SERVICE_MIN_M: f64 = 60.0;
    /// Probability (per mille) of a tall building (4 to 8 floors); higher on primary/secondary.
    pub const TALL_PERMILLE: u64 = 200;
    pub const TALL_MAIN_PERMILLE: u64 = 400;

    const _: () = assert!(LOT_W_MIN_M < LOT_W_MAX_M && LOT_D_MIN_M < LOT_D_MAX_M);
    const _: () = assert!(LOT_W_MIN_M % MODULE_M == 0.0 && LOT_D_MIN_M % MODULE_M == 0.0);
    const _: () = assert!(FRONT_SETBACK_MAX_M % MODULE_M == 0.0 && SIDE_SETBACK_MAX_M % MODULE_M == 0.0);
    const _: () = assert!(FOOTPRINT_MARGIN_M % MODULE_M == 0.0);
    const _: () = assert!(EMPTY_LOT_PERMILLE < 1000 && TALL_PERMILLE <= TALL_MAIN_PERMILLE && TALL_MAIN_PERMILLE < 1000);
    // A lot must still hold a building after both setbacks on each side.
    const _: () = assert!(LOT_W_MIN_M > 2.0 * SIDE_SETBACK_MAX_M && LOT_D_MIN_M > FRONT_SETBACK_MAX_M + SIDE_SETBACK_MAX_M);
}

/// Widths of the waterway centerlines in the ground map (`river`, `canal`/`drain`, `stream`), in meters.
pub const WATERWAY_RIVER_M: f64 = 8.0;
pub const WATERWAY_CANAL_M: f64 = 4.0;
pub const WATERWAY_STREAM_M: f64 = 2.0;
