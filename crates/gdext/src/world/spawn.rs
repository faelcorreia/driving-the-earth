//! Spawn pose on the road nearest to home, facing south.
//!
//! Local coordinates: `x` east, `y` north (see `world::map`). The car's forward is -Z in Godot,
//! which is north, so a car facing south has its forward at +Z. Yaw is the Godot rotation
//! about +Y that turns -Z into the wanted heading.
use std::io;

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

/// Picks the road segment closest to the origin that has a north-south component, and
/// returns its projection of the origin with a yaw facing south along that segment.
///
/// `tiles` are the map tiles; `None` when there is no usable road segment.
pub fn road_spawn(tiles: &[Tile]) -> Option<SpawnPose> {
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
                let t = (-(p[0] * d[0] + p[1] * d[1]) / len2).clamp(0.0, 1.0);
                let c = [p[0] + t * d[0], p[1] + t * d[1]];
                let dist2 = c[0] * c[0] + c[1] * c[1];
                if best.is_none_or(|(b, _, _)| dist2 < b) {
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
    let tile_m = index.meta.tile_size_m;
    let near: Vec<Tile> = index
        .coords()
        .iter()
        .filter(|&&t| distance_to_tile(t, tile_m, [0.0, 0.0]) <= search_m)
        .map(|&(x, y)| index.read_tile_at(x, y))
        .collect::<io::Result<_>>()?;
    if let Some(pose) = road_spawn(&near) {
        if pose.x.hypot(pose.y) <= search_m {
            return Ok(Some(pose));
        }
    }
    let all: Vec<Tile> = index
        .coords()
        .iter()
        .map(|&(x, y)| index.read_tile_at(x, y))
        .collect::<io::Result<_>>()?;
    Ok(road_spawn(&all))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tileformat::{HeightGrid, Polyline};

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
