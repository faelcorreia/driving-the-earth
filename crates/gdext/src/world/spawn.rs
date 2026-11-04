//! Spawn pose on the road nearest to home, facing south.
//!
//! Local coordinates: `x` east, `y` north (see `world::map`). The car's forward is -Z in Godot,
//! which is north, so a car facing south has its forward at +Z. Yaw is the Godot rotation
//! about +Y that turns -Z into the wanted heading.
use std::io;

use tilebake::dem::MapGrid;
use tileformat::Tile;

use super::map::MapIndex;
use super::streaming::distance_to_tile;

/// Segments shorter than this, in meters, are ignored.
const MIN_SEGMENT_M: f64 = 1e-9;
/// A segment must move at least this far north or south to have a south-facing direction.
const MIN_NS_COMPONENT_M: f64 = 1e-6;

/// Where the car is born: a point on a road in local meters, and the Godot yaw in radians.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpawnPose {
    pub x: f64,
    pub y: f64,
    pub yaw: f32,
}

/// The road segment closest to `from` (local meters) that has a north-south component, with the
/// projection of `from` on it and a yaw facing south along the segment. `None` when there is no
/// usable road segment in `tiles`.
pub fn road_spawn_near(tiles: &[Tile], from: [f64; 2]) -> Option<SpawnPose> {
    let mut best: Option<(f64, [f64; 2], [f64; 2])> = None;

    for tile in tiles {
        for road in &tile.roads {
            for seg in road.points.windows(2) {
                let (p, q) = (seg[0], seg[1]);
                let d = [q[0] - p[0], q[1] - p[1]];
                let len2 = d[0] * d[0] + d[1] * d[1];
                if len2.sqrt() < MIN_SEGMENT_M || d[1].abs() < MIN_NS_COMPONENT_M {
                    continue;
                }
                let r = [p[0] - from[0], p[1] - from[1]];
                let t = (-(r[0] * d[0] + r[1] * d[1]) / len2).clamp(0.0, 1.0);
                let c = [p[0] + t * d[0], p[1] + t * d[1]];
                let dist2 = (c[0] - from[0]).powi(2) + (c[1] - from[1]).powi(2);
                if best.map_or(true, |(b, _, _)| dist2 < b) {
                    best = Some((dist2, c, d));
                }
            }
        }
    }

    let (_, c, d) = best?;
    // Pick the direction of the segment that moves south (negative local y).
    let sign = if d[1] < 0.0 { 1.0 } else { -1.0 };
    let (fx, fy) = (sign * d[0], sign * d[1]);
    // Godot forward is (fx, 0, -fy); yaw turns -Z into it: forward = (-sin yaw, 0, -cos yaw).
    let yaw = (-fx).atan2(fy);
    Some(SpawnPose {
        x: c[0],
        y: c[1],
        yaw: yaw as f32,
    })
}

/// The road nearest to home, searched among the tiles within `search_m` of it. Falls back
/// to reading every tile when the nearest road is farther than that.
pub fn find_spawn(index: &MapIndex, search_m: f64) -> io::Result<Option<SpawnPose>> {
    find_spawn_near(index, [0.0, 0.0], search_m)
}

/// The road nearest to `from` (local meters), searched among the tiles within `search_m` of it.
/// Falls back to reading every tile when the nearest road is farther than that.
pub fn find_spawn_near(
    index: &MapIndex,
    from: [f64; 2],
    search_m: f64,
) -> io::Result<Option<SpawnPose>> {
    let tile_m = index.meta.tile_size_m;
    let near: Vec<Tile> = index
        .coords()
        .iter()
        .filter(|&&t| distance_to_tile(t, tile_m, from) <= search_m)
        .map(|&(x, y)| index.read_tile_at(x, y))
        .collect::<io::Result<_>>()?;
    if let Some(pose) = road_spawn_near(&near, from) {
        if (pose.x - from[0]).hypot(pose.y - from[1]) <= search_m {
            return Ok(Some(pose));
        }
    }
    let all: Vec<Tile> = index
        .coords()
        .iter()
        .map(|&(x, y)| index.read_tile_at(x, y))
        .collect::<io::Result<_>>()?;
    Ok(road_spawn_near(&all, from))
}

/// Local meters `(x, y)` of `(lat, lon)` when the point is inside the baked map: within
/// `radius_m` of the map center and on a tile that is present (`has_tile` takes the tile index).
/// `None` when the point is outside, so the caller must bake that region first.
pub fn inside_loaded_map(
    grid: &MapGrid,
    radius_m: f64,
    lat: f64,
    lon: f64,
    has_tile: impl Fn((i32, i32)) -> bool,
) -> Option<[f64; 2]> {
    let (x, y) = grid.local(lat, lon);
    if !(x.is_finite() && y.is_finite()) || x.hypot(y) > radius_m {
        return None;
    }
    let tile = (
        (x / grid.tile_m).floor() as i32,
        (y / grid.tile_m).floor() as i32,
    );
    has_tile(tile).then_some([x, y])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tileformat::{HeightGrid, Polyline};

    fn road_spawn(tiles: &[Tile]) -> Option<SpawnPose> {
        road_spawn_near(tiles, [0.0, 0.0])
    }

    fn tile_with_roads(roads: Vec<Vec<[f64; 2]>>) -> Tile {
        Tile {
            x: 0,
            y: 0,
            height: HeightGrid {
                cols: 2,
                rows: 2,
                values: vec![0.0; 4],
            },
            roads: roads
                .into_iter()
                .map(|points| Polyline {
                    name: String::new(),
                    z: Vec::new(),
                    zl: Vec::new(),
                    zr: Vec::new(),
                    highway: "residential".into(),
                    points,
                })
                .collect(),
            buildings: vec![],
            ground: vec![],
        }
    }

    /// Forward vector in local (east, north) for a yaw, to check the heading.
    fn heading(yaw: f32) -> (f64, f64) {
        let y = yaw as f64;
        // Godot forward (-sin yaw, 0, -cos yaw): east = -sin, north = -(z) = cos.
        (-y.sin(), y.cos())
    }

    #[test]
    fn faces_south_along_nearest_road() {
        // A north-south road east of home, 5 m away; the segment goes north.
        let tile = tile_with_roads(vec![vec![[5.0, -10.0], [5.0, 10.0]]]);
        let pose = road_spawn(&[tile]).unwrap();
        let (east, north) = heading(pose.yaw);
        assert!(north < -0.999, "north = {north}");
        assert!(east.abs() < 1e-6, "east = {east}");
        assert!((pose.x - 5.0).abs() < 1e-9 && pose.y.abs() < 1e-9);
    }

    #[test]
    fn skips_east_west_segments() {
        // Closest segment is east-west (no south component); the farther one is used.
        let tile = tile_with_roads(vec![
            vec![[-1.0, 1.0], [1.0, 1.0]],
            vec![[0.0, 30.0], [0.0, 20.0]],
        ]);
        let pose = road_spawn(&[tile]).unwrap();
        assert!((pose.y - 20.0).abs() < 1e-9 && pose.x.abs() < 1e-9);
        let (_, north) = heading(pose.yaw);
        assert!(north < -0.999, "north = {north}");
    }

    #[test]
    fn projects_origin_onto_segment() {
        // Segment passes at x = 3 with a slope; the projection is the closest point.
        let tile = tile_with_roads(vec![vec![[3.0, 4.0], [3.0, -4.0]]]);
        let pose = road_spawn(&[tile]).unwrap();
        assert!((pose.x - 3.0).abs() < 1e-9 && pose.y.abs() < 1e-9);
    }

    #[test]
    fn none_without_roads() {
        assert_eq!(road_spawn(&[tile_with_roads(vec![])]), None);
        assert_eq!(road_spawn(&[]), None);
    }
}

#[cfg(test)]
mod point_tests {
    use super::*;
    use tileformat::Polyline;

    fn road(points: Vec<[f64; 2]>) -> Tile {
        Tile {
            x: 0,
            y: 0,
            height: tileformat::HeightGrid {
                cols: 2,
                rows: 2,
                values: vec![0.0; 4],
            },
            roads: vec![Polyline {
                name: String::new(),
                z: Vec::new(),
                zl: Vec::new(),
                zr: Vec::new(),
                highway: "residential".into(),
                points,
            }],
            buildings: vec![],
            ground: vec![],
        }
    }

    #[test]
    fn snaps_to_the_road_nearest_to_the_point() {
        // Two north-south roads: x = 100 (near the point) and x = -500 (near the origin).
        let tiles = [
            road(vec![[-500.0, -50.0], [-500.0, 50.0]]),
            road(vec![[100.0, -50.0], [100.0, 50.0]]),
        ];
        let pose = road_spawn_near(&tiles, [90.0, 10.0]).unwrap();
        assert!((pose.x - 100.0).abs() < 1e-9);
        assert!((pose.y - 10.0).abs() < 1e-9);
        // Near (-480, 0) the other road wins; the origin's nearest road is x = 100 (distance 100).
        let west = road_spawn_near(&tiles, [-480.0, 0.0]).unwrap();
        assert!((west.x + 500.0).abs() < 1e-9);
        let origin = road_spawn_near(&tiles, [0.0, 0.0]).unwrap();
        assert!((origin.x - 100.0).abs() < 1e-9);
    }

    #[test]
    fn projects_the_point_onto_the_segment_ends() {
        let tiles = [road(vec![[0.0, 0.0], [0.0, 100.0]])];
        let pose = road_spawn_near(&tiles, [0.0, 300.0]).unwrap();
        assert!((pose.y - 100.0).abs() < 1e-9, "clamped to the segment end");
    }

    fn grid() -> MapGrid {
        MapGrid {
            lat0: -19.9,
            lon0: -43.9,
            tile_m: 100.0,
            cell_m: 1.0,
        }
    }

    #[test]
    fn point_inside_radius_with_tile_is_local() {
        let g = grid();
        let (lat, lon) = g.latlon(250.0, -130.0);
        let got = inside_loaded_map(&g, 1000.0, lat, lon, |_| true).unwrap();
        assert!((got[0] - 250.0).abs() < 1e-6 && (got[1] + 130.0).abs() < 1e-6);
    }

    #[test]
    fn point_outside_radius_or_without_tile_is_none() {
        let g = grid();
        let (lat, lon) = g.latlon(1500.0, 0.0);
        assert!(inside_loaded_map(&g, 1000.0, lat, lon, |_| true).is_none());
        let (lat, lon) = g.latlon(250.0, 0.0);
        assert!(inside_loaded_map(&g, 1000.0, lat, lon, |_| false).is_none());
        // The tile check uses the tile containing the point: (2, 0) for x = 250.
        assert!(inside_loaded_map(&g, 1000.0, lat, lon, |t| t == (2, 0)).is_some());
    }
}
