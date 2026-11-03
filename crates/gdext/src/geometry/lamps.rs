//! Procedural street lamps (L1): where the poles stand along the urban roads of a tile.
//! Pure Rust, no Godot. Positions are world meters (f64), like the tile data.
//! Rules: one lamp every `LAMP_SPACING_M` along a lamp road, alternating sides (both sides on
//! primary and secondary), `LAMP_SIDE_OFFSET_M` beyond the carriageway edge, the arm reaching
//! `LAMP_ARM_M` over the carriageway; nothing inside a building footprint and nothing closer
//! than `LAMP_CROSSING_CLEAR_M` to another road axis of the tile.

use tileformat::units::lamps::{LAMP_CROSSING_CLEAR_M, LAMP_SERVICE_MIN_M, LAMP_SIDE_OFFSET_M, LAMP_SPACING_M};
use tileformat::{road_width_m, Polygon, Polyline, Tile};

/// One street lamp: the pole base on the ground and the unit direction of its arm.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)] // no consumer until the lamp render step (L1)
pub struct Lamp {
    /// Stable id: a hash of the road key and the lamp index along it.
    pub id: u64,
    pub base: [f64; 2],
    pub arm_dir: [f64; 2],
}

/// Whether a road of this OSM `highway` class gets lamps. Service roads only when longer than
/// `LAMP_SERVICE_MIN_M` (the length given is the one of the road piece in the tile).
pub fn is_lamp_road(highway: &str, length_m: f64) -> bool {
    match highway {
        "residential" | "tertiary" | "secondary" | "primary" | "unclassified" | "living_street" => true,
        "service" => length_m > LAMP_SERVICE_MIN_M,
        _ => false,
    }
}

/// Lamps of every road of the tile. The road index is the key that seeds the ids.
#[allow(dead_code)] // no consumer until the lamp render step (L1)
pub fn lamps_of_tile(tile: &Tile) -> Vec<Lamp> {
    let mut out = Vec::new();
    for (idx, road) in tile.roads.iter().enumerate() {
        let others: Vec<&Polyline> = tile.roads.iter().enumerate().filter(|(j, _)| *j != idx).map(|(_, r)| r).collect();
        out.extend(lamps_of_road(idx as u64, road, &others, &tile.buildings));
    }
    out
}

/// Lamps of one road. `others` are the other road axes of the tile (crossings) and `buildings`
/// the footprints of the tile. Output order: along the road, then by side.
pub fn lamps_of_road(road_key: u64, road: &Polyline, others: &[&Polyline], buildings: &[Polygon]) -> Vec<Lamp> {
    let length = polyline_length(&road.points);
    if !is_lamp_road(&road.highway, length) {
        return Vec::new();
    }
    let reach = road_width_m(&road.highway) / 2.0 + LAMP_SIDE_OFFSET_M;
    let both = matches!(road.highway.as_str(), "primary" | "secondary");
    let mut out = Vec::new();
    let mut k = 0u64;
    loop {
        let s = (k as f64 + 0.5) * LAMP_SPACING_M;
        if s > length {
            break;
        }
        if let Some((pos, tan)) = point_at(&road.points, s) {
            let normal = [-tan[1], tan[0]];
            // Alternating sides: +1 on even samples, -1 on odd ones; avenues use both.
            let sides: &[f64] = if both {
                &[1.0, -1.0]
            } else if k % 2 == 0 {
                &[1.0]
            } else {
                &[-1.0]
            };
            for &side in sides {
                let base = [pos[0] + side * normal[0] * reach, pos[1] + side * normal[1] * reach];
                let arm_dir = [-side * normal[0], -side * normal[1]];
                let index = k * 2 + u64::from(side < 0.0);
                if inside_any(base, buildings) || near_other_axis(base, others) {
                    continue;
                }
                out.push(Lamp { id: lamp_id(road_key, index), base, arm_dir });
            }
        }
        k += 1;
    }
    out
}

/// Stable id of a lamp: a hash of the road key and the lamp index (no state, no ordering).
pub fn lamp_id(road_key: u64, index: u64) -> u64 {
    mix(mix(road_key) ^ index.wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// splitmix64 finalizer.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn polyline_length(points: &[[f64; 2]]) -> f64 {
    points.windows(2).map(|w| dist(w[0], w[1])).sum()
}

/// Point and unit tangent at arc length `s` along the polyline, or `None` past its end.
fn point_at(points: &[[f64; 2]], s: f64) -> Option<([f64; 2], [f64; 2])> {
    let mut acc = 0.0;
    for w in points.windows(2) {
        let len = dist(w[0], w[1]);
        if len <= 0.0 {
            continue;
        }
        if acc + len >= s {
            let t = (s - acc) / len;
            let tan = [(w[1][0] - w[0][0]) / len, (w[1][1] - w[0][1]) / len];
            let pos = [w[0][0] + t * (w[1][0] - w[0][0]), w[0][1] + t * (w[1][1] - w[0][1])];
            return Some((pos, tan));
        }
        acc += len;
    }
    None
}

fn inside_any(p: [f64; 2], polygons: &[Polygon]) -> bool {
    polygons.iter().any(|poly| point_in_ring(p, &poly.points))
}

/// Even-odd ray casting.
fn point_in_ring(p: [f64; 2], ring: &[[f64; 2]]) -> bool {
    let mut inside = false;
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (ring[i], ring[j]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn near_other_axis(p: [f64; 2], others: &[&Polyline]) -> bool {
    others.iter().any(|r| {
        r.points
            .windows(2)
            .any(|w| dist_to_segment(p, w[0], w[1]) < LAMP_CROSSING_CLEAR_M)
    })
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

fn dist_to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    dist(p, [a[0] + t * dx, a[1] + t * dy])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn road(highway: &str, points: &[[f64; 2]]) -> Polyline {
        Polyline { highway: highway.into(), points: points.to_vec(), z: vec![], zl: vec![], zr: vec![] }
    }

    fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
        Polygon::real(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
    }

    /// Straight road along x from 0 to `len` m.
    fn straight(highway: &str, len: f64) -> Polyline {
        road(highway, &[[0.0, 0.0], [len, 0.0]])
    }

    #[test]
    fn spacing_and_alternating_sides() {
        let r = straight("residential", 350.0);
        let lamps = lamps_of_road(0, &r, &[], &[]);
        // Samples at 17.5, 52.5, ... 332.5: ten lamps, 35 m apart.
        assert_eq!(lamps.len(), 10);
        for w in lamps.windows(2) {
            assert!((w[1].base[0] - w[0].base[0] - LAMP_SPACING_M).abs() < 1e-9);
            assert!(w[0].base[1] * w[1].base[1] < 0.0, "sides alternate");
        }
        // Residential: half width 3 m + 1 m offset.
        assert!(lamps.iter().all(|l| (l.base[1].abs() - 4.0).abs() < 1e-9));
    }

    #[test]
    fn arm_points_over_the_carriageway() {
        for l in lamps_of_road(0, &straight("residential", 100.0), &[], &[]) {
            assert_eq!(l.arm_dir[1], -l.base[1].signum());
            assert_eq!(l.arm_dir[0], 0.0);
        }
    }

    #[test]
    fn avenues_light_both_sides_and_service_needs_length() {
        assert_eq!(lamps_of_road(0, &straight("primary", 350.0), &[], &[]).len(), 20);
        assert!(lamps_of_road(0, &straight("service", 50.0), &[], &[]).is_empty());
        assert!(!lamps_of_road(0, &straight("service", 100.0), &[], &[]).is_empty());
        assert!(lamps_of_road(0, &straight("footway", 300.0), &[], &[]).is_empty());
    }

    #[test]
    fn no_lamp_inside_a_building_footprint() {
        let r = straight("residential", 350.0);
        let all = lamps_of_road(0, &r, &[], &[]);
        let b = [square(10.0, 0.0, 25.0, 10.0)];
        let lamps = lamps_of_road(0, &r, &[], &b);
        assert_eq!(lamps.len(), all.len() - 1, "only the lamp at 17.5 m is inside");
        assert!(lamps.iter().all(|l| !inside_any(l.base, &b)));
    }

    #[test]
    fn no_lamp_near_a_crossing() {
        let r = straight("residential", 350.0);
        // Crossing road on x = 52.5 m, where a lamp would stand.
        let cross = road("residential", &[[52.5, -50.0], [52.5, 50.0]]);
        let lamps = lamps_of_road(0, &r, &[&cross], &[]);
        assert!(lamps.iter().all(|l| dist_to_segment(l.base, [52.5, -50.0], [52.5, 50.0]) >= LAMP_CROSSING_CLEAR_M));
        assert_eq!(lamps.len(), 9);
    }

    #[test]
    fn deterministic_and_stable_ids() {
        let r = straight("tertiary", 300.0);
        let a = lamps_of_road(7, &r, &[], &[]);
        assert_eq!(a, lamps_of_road(7, &r, &[], &[]));
        let mut ids: Vec<u64> = a.iter().map(|l| l.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), a.len(), "ids are unique within a road");
        // Another road key gives other ids; the same lamp keeps its id when a building removes a neighbour.
        assert_ne!(a[0].id, lamps_of_road(8, &r, &[], &[])[0].id);
        let b = [square(10.0, 0.0, 25.0, 10.0)];
        let fewer = lamps_of_road(7, &r, &[], &b);
        assert!(fewer.iter().all(|l| a.iter().any(|x| x.id == l.id)));
    }
}
