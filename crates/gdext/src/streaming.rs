//! Which tiles to keep loaded around the car. Pure geometry, no Godot types.
//!
//! Tile `(x, y)` covers the square `[x * s, (x + 1) * s] x [y * s, (y + 1) * s]` in local
//! meters, with `s` the tile size. Points are `[x, y]` in the same local meters.

/// Distance from `p` to the square of `tile`; zero when `p` is inside it.
pub fn distance_to_tile(tile: (i32, i32), tile_m: f64, p: [f64; 2]) -> f64 {
    let (x0, y0) = (tile.0 as f64 * tile_m, tile.1 as f64 * tile_m);
    let dx = (x0 - p[0]).max(p[0] - (x0 + tile_m)).max(0.0);
    let dy = (y0 - p[1]).max(p[1] - (y0 + tile_m)).max(0.0);
    dx.hypot(dy)
}

/// The tiles of `all` within `radius_m` of `p`, nearest first.
pub fn tiles_in_range(all: &[(i32, i32)], tile_m: f64, p: [f64; 2], radius_m: f64) -> Vec<(i32, i32)> {
    let mut hits: Vec<((i32, i32), f64)> = all
        .iter()
        .map(|&t| (t, distance_to_tile(t, tile_m, p)))
        .filter(|&(_, d)| d <= radius_m)
        .collect();
    hits.sort_by(|a, b| a.1.total_cmp(&b.1));
    hits.into_iter().map(|(t, _)| t).collect()
}

/// A loaded tile is unloaded only once it is beyond `radius_m + hysteresis_m`, so a tile at the
/// edge of the view does not flicker in and out while the car moves along it.
pub fn is_stale(tile: (i32, i32), tile_m: f64, p: [f64; 2], radius_m: f64, hysteresis_m: f64) -> bool {
    distance_to_tile(tile, tile_m, p) > radius_m + hysteresis_m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_is_zero_inside_and_grows_outside() {
        // Tile (2, -1) with 100 m tiles covers x in [200, 300], y in [-100, 0].
        assert_eq!(distance_to_tile((2, -1), 100.0, [250.0, -50.0]), 0.0);
        assert_eq!(distance_to_tile((2, -1), 100.0, [200.0, 0.0]), 0.0);
        assert_eq!(distance_to_tile((2, -1), 100.0, [150.0, -50.0]), 50.0);
        assert_eq!(distance_to_tile((2, -1), 100.0, [250.0, 30.0]), 30.0);
    }

    #[test]
    fn distance_to_a_corner_is_euclidean() {
        assert_eq!(distance_to_tile((0, 0), 100.0, [103.0, 104.0]), 5.0);
        assert_eq!(distance_to_tile((0, 0), 100.0, [-3.0, -4.0]), 5.0);
    }

    #[test]
    fn tiles_in_range_keeps_the_radius_and_sorts_nearest_first() {
        let all = [(0, 0), (1, 0), (5, 5), (-1, 0), (0, 4)];
        // Car at (50, 50) in 100 m tiles: (0, 0) contains it, (1, 0) and (-1, 0) are 50 m away,
        // (0, 4) is 350 m away and (5, 5) is far.
        let got = tiles_in_range(&all, 100.0, [50.0, 50.0], 60.0);
        assert_eq!(got, vec![(0, 0), (1, 0), (-1, 0)]);
    }

    #[test]
    fn tiles_in_range_is_empty_when_nothing_is_close() {
        assert!(tiles_in_range(&[(10, 10)], 100.0, [0.0, 0.0], 500.0).is_empty());
    }

    #[test]
    fn hysteresis_keeps_tiles_between_radius_and_radius_plus_margin() {
        // Tile (3, 0) starts at x = 300; the car at x = 0 is 300 m away.
        let (tile, car) = ((3, 0), [0.0, 50.0]);
        assert!(!is_stale(tile, 100.0, car, 300.0, 50.0));
        assert!(!is_stale(tile, 100.0, car, 250.0, 50.0));
        assert!(is_stale(tile, 100.0, car, 240.0, 50.0));
        // Inside the view: never stale.
        assert!(!is_stale((0, 0), 100.0, car, 10.0, 0.0));
    }

    #[test]
    fn negative_coordinates_are_handled() {
        assert_eq!(distance_to_tile((-1, -1), 100.0, [-50.0, -50.0]), 0.0);
        assert_eq!(distance_to_tile((-1, -1), 100.0, [5.0, -50.0]), 5.0);
    }
}
