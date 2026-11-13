//! Road geometry: ribbons that sample the terrain surface field of the tile.
//! Pure Rust (no Godot types). Imports `geometry::{mesh, vec2, field}` and `tileformat` units.
use tileformat::units::ROAD_STATION_M;

use super::field::Surface;
use super::mesh::{push_up_facing, world, MeshData};
use super::vec2::{dist, miter, scale, unit_normal};

/// Height of the road ribbons above the terrain surface, in meters (avoids z-fighting).
pub(crate) const ROAD_LIFT_M: f32 = 0.1;
/// Extra lift per road index (modulo 8), so overlapping ribbons do not z-fight.
pub(super) const ROAD_ORDER_LIFT_M: f32 = 0.002;
/// Lift of the ribbon's left and right edges above the terrain, in meters. The center keeps the
/// full lift, so the road is a crown that falls to the terrain at its edges: no lip for a wheel or
/// the chassis to catch on when the car leaves the road.
pub(crate) const ROAD_EDGE_LIFT_M: f32 = 0.01;
/// Extra ribbon lift per meter of height step to the neighboring stations beyond
/// `ROAD_STEP_FREE_M` (about 6% grade at 2.5 m stations; gentler stretches keep the plain lift), capped.
const ROAD_STEP_FREE_M: f32 = 0.15;
const ROAD_STEP_LIFT: f32 = 0.4;
const ROAD_STEP_LIFT_MAX_M: f32 = 0.4;
const ROAD_STEP_CLIFF_M: f32 = 1.5;

/// One road as a continuous strip, built but not yet written to a mesh: three vertices per
/// station (left edge, center, right edge), two quads between stations. Stations sit at the
/// polyline nodes and every `ROAD_STATION_M` meters in between. Every vertex takes the height of
/// the terrain field at its own position, plus the road's `lift` and the anti-poke-through lift
/// (`extra`). At each node the two segment normals are joined (miter), so bends have no gaps.
///
/// Building the strip apart from the mesh lets the junction reconciliation give every road that
/// meets at a shared node the same lift at that node (see `junction::reconcile_lift`), so the
/// ribbons meet at one height there instead of one stepping over the other.
pub(super) struct Ribbon {
    /// Station centers, one per station, in the same order as `extra`.
    centers: Vec<[f64; 2]>,
    /// Left edge, center and right edge of every station, three per station.
    xy: Vec<[f64; 2]>,
    /// Sampled field height at each of `xy`, three per station.
    heights: Vec<f32>,
    /// Anti-poke-through lift per station (see `smooth_extra`).
    extra: Vec<f32>,
    /// Station indices of the polyline points: the only places the bake calls a node.
    nodes: Vec<usize>,
}

impl Ribbon {
    /// `None` when the road has fewer than two distinct points (nothing to draw).
    pub(super) fn new(
        surface: &Surface,
        ground: &dyn Fn(f64, f64) -> f32,
        points: &[[f64; 2]],
        half: f64,
    ) -> Option<Ribbon> {
        // Drop repeated points: they have no direction.
        let mut pts: Vec<[f64; 2]> = Vec::with_capacity(points.len());
        for &p in points {
            match pts.last() {
                Some(&q) if dist(p, q) <= 1e-9 => {}
                _ => pts.push(p),
            }
        }
        if pts.len() < 2 {
            return None;
        }
        let normals: Vec<[f64; 2]> = pts.windows(2).map(|w| unit_normal(w[0], w[1])).collect();

        // Offset from the center to the left edge, one per station, and the station position.
        // `nodes` remembers which stations are polyline points: the bake reconciles heights only
        // at those, so the junction weld matches the same places.
        let mut stations: Vec<([f64; 2], [f64; 2])> = Vec::new();
        let mut nodes: Vec<usize> = Vec::new();
        nodes.push(0);
        stations.push((pts[0], scale(normals[0], half)));
        for k in 0..pts.len() - 1 {
            let (p, q) = (pts[k], pts[k + 1]);
            let pieces = (dist(p, q) / ROAD_STATION_M).ceil().max(1.0) as usize;
            for s in 1..pieces {
                let t = s as f64 / pieces as f64;
                let at = [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t];
                stations.push((at, scale(normals[k], half)));
            }
            let offset = match normals.get(k + 1) {
                Some(&next) => miter(normals[k], next, half),
                None => scale(normals[k], half),
            };
            nodes.push(stations.len());
            stations.push((q, offset));
        }

        let mut centers = Vec::with_capacity(stations.len());
        let mut xy: Vec<[f64; 2]> = Vec::with_capacity(stations.len() * 3);
        let mut heights: Vec<f32> = Vec::with_capacity(stations.len() * 3);
        for &(c, o) in &stations {
            centers.push(c);
            for [x, y] in [[c[0] + o[0], c[1] + o[1]], c, [c[0] - o[0], c[1] - o[1]]] {
                xy.push([x, y]);
                heights.push(surface.height(ground, x, y));
            }
        }
        // The ribbon is flat between its stations and the terrain between its grid vertices, with
        // different triangles; on a steep stretch they differ by a fraction of the height step
        // between neighbors. The ribbon rises by that fraction so the terrain never shows through.
        let n = centers.len();
        let raw: Vec<f32> = (0..n)
            .map(|s| {
                let mut step = 0.0f32;
                for c in 0..3 {
                    let h = heights[3 * s + c];
                    if s > 0 {
                        step = step.max((h - heights[3 * (s - 1) + c]).abs());
                    }
                    if s + 1 < n {
                        step = step.max((h - heights[3 * (s + 1) + c]).abs());
                    }
                }
                step = step
                    .max((heights[3 * s] - heights[3 * s + 1]).abs())
                    .max((heights[3 * s + 1] - heights[3 * s + 2]).abs());
                // A step beyond `ROAD_STEP_CLIFF_M` is a cliff (a tile border over a different terrain), not a slope.
                if step > ROAD_STEP_CLIFF_M {
                    0.0
                } else {
                    (ROAD_STEP_LIFT * (step - ROAD_STEP_FREE_M).max(0.0)).min(ROAD_STEP_LIFT_MAX_M)
                }
            })
            .collect();
        let extra = smooth_extra(&raw);
        Some(Ribbon {
            centers,
            xy,
            heights,
            extra,
            nodes,
        })
    }

    /// Station centers, in the same order as `extra` and the strip vertices.
    pub(super) fn centers(&self) -> &[[f64; 2]] {
        &self.centers
    }

    /// Station indices of the polyline points (the places the bake treats as nodes).
    pub(super) fn node_stations(&self) -> &[usize] {
        &self.nodes
    }

    /// Anti-poke-through lift at station `s`, in meters.
    pub(super) fn extra_at(&self, s: usize) -> f32 {
        self.extra[s]
    }

    /// Field height of the center vertex at station `s`, before any lift: what a junction weld
    /// compares to give the roads meeting there one height.
    pub(super) fn center_height(&self, s: usize) -> f32 {
        self.heights[3 * s + 1]
    }

    /// Raises the lift around `station` to `value`, easing down at `EXTRA_STEP_LIMIT_M` per
    /// station. Only raises: no ribbon is pushed down toward the terrain. Used to give every
    /// road meeting at a junction the same lift at the shared node.
    pub(super) fn raise_at(&mut self, station: usize, value: f32) {
        // Beyond this many stations the cone is already below any lift it could raise.
        let span = (value / EXTRA_STEP_LIMIT_M).ceil() as usize + 1;
        let lo = station.saturating_sub(span);
        let hi = (station + span + 1).min(self.extra.len());
        for s in lo..hi {
            let d = (s as f32 - station as f32).abs() * EXTRA_STEP_LIMIT_M;
            self.extra[s] = self.extra[s].max(value - d);
        }
    }

    /// Writes the strip, raising every vertex by the anti-poke-through lift and by `lift` (the
    /// road's base lift). Edge columns take the edge lift, the center column the full lift.
    pub(super) fn push(&self, mesh: &mut MeshData, lift: f32) {
        let n = self.centers.len();
        let base = mesh.vertices.len() as u32;
        let edge_lift = lift - ROAD_LIFT_M + ROAD_EDGE_LIFT_M;
        for s in 0..n {
            for c in 0..3 {
                let [x, y] = self.xy[3 * s + c];
                let base_lift = if c == 1 { lift } else { edge_lift };
                mesh.vertices.push(world(
                    x,
                    y,
                    self.heights[3 * s + c] + base_lift + self.extra[s],
                ));
                mesh.normals.push([0.0, 1.0, 0.0]);
            }
        }
        for s in 0..n - 1 {
            // Vertex of station s on the left (3s), center (3s + 1) and right (3s + 2) edge.
            let l0 = base + 3 * s as u32;
            let l1 = l0 + 3;
            push_up_facing(mesh, l0, l0 + 1, l1 + 1);
            push_up_facing(mesh, l0, l1 + 1, l1);
            push_up_facing(mesh, l0 + 1, l0 + 2, l1 + 2);
            push_up_facing(mesh, l0 + 1, l1 + 2, l1 + 1);
        }
    }
}

/// Largest change of the lift between neighboring stations, in meters.
const EXTRA_STEP_LIMIT_M: f32 = 0.02;

/// Smallest lift that keeps the ribbon above `raw` and changes by at most `EXTRA_STEP_LIMIT_M`
/// per station: the upper envelope of the cones `raw[j] - EXTRA_STEP_LIMIT_M * |s - j|`, computed
/// by a forward and a backward pass.
fn smooth_extra(raw: &[f32]) -> Vec<f32> {
    let mut env = raw.to_vec();
    for s in 1..env.len() {
        env[s] = env[s].max(env[s - 1] - EXTRA_STEP_LIMIT_M);
    }
    for s in (0..env.len().saturating_sub(1)).rev() {
        env[s] = env[s].max(env[s + 1] - EXTRA_STEP_LIMIT_M);
    }
    env
}

#[cfg(test)]
mod tests {
    use super::super::tests::{grid, ribbon_height, tile};
    use super::super::tile_geometry_with;
    use std::f64::consts::FRAC_1_SQRT_2;
    use tileformat::Polyline;

    fn road(points: Vec<[f64; 2]>) -> Polyline {
        Polyline {
            oneway: None,
            lanes: None,
            maxspeed: None,
            roundabout: false,
            name: String::new(),
            z: Vec::new(),
            zl: Vec::new(),
            zr: Vec::new(),
            highway: "residential".into(),
            points,
            lit: None,
        }
    }

    /// Distance from `p` to the polyline `pts`.
    fn dist_to_polyline(p: [f64; 2], pts: &[[f64; 2]]) -> f64 {
        pts.windows(2)
            .map(|w| {
                let (a, b) = (w[0], w[1]);
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (dx * dx + dy * dy))
                    .clamp(0.0, 1.0);
                let (cx, cy) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
                (cx * cx + cy * cy).sqrt()
            })
            .fold(f64::INFINITY, f64::min)
    }

    /// A 90 degree corner exactly on the seam x = 250 m: one leg in each tile, each tile holds
    /// its own piece of the road. The outer side of the corner must not be left uncovered
    /// by either tile's ribbon.
    #[test]
    fn right_angle_corner_on_tile_seam_leaves_no_notch() {
        let s = FRAC_1_SQRT_2;
        let corner = [250.0, 125.0];
        let a = [corner[0] - 80.0 * s, corner[1] + 80.0 * s];
        let b = [corner[0] + 80.0 * s, corner[1] + 80.0 * s];
        let left = tile(0, 0, grid(26, |_, _| 50.0), vec![road(vec![a, corner])]);
        let right = tile(1, 0, grid(26, |_, _| 50.0), vec![road(vec![corner, b])]);
        let g_left = tile_geometry_with(&left, 250.0, &|_, _| 50.0, &[&right]);
        let g_right = tile_geometry_with(&right, 250.0, &|_, _| 50.0, &[&left]);

        let half = 3.0;
        let centerline = [a, corner, b];
        let mut missing = Vec::new();
        let mut checked = 0;
        // Every point within the ribbon (clear of its edge) near the corner, on either tile's mesh.
        for i in -60..=60 {
            for j in -60..=60 {
                let x = corner[0] + i as f64 * 0.1;
                let y = corner[1] + j as f64 * 0.1;
                if dist_to_polyline([x, y], &centerline) > half - 0.2 {
                    continue;
                }
                checked += 1;
                let covered = ribbon_height(&g_left.roads, x, y).is_some()
                    || ribbon_height(&g_right.roads, x, y).is_some();
                if !covered {
                    missing.push((x, y));
                }
            }
        }
        assert!(checked > 500, "only {checked} points checked");
        assert!(
            missing.is_empty(),
            "{} uncovered points at the seam corner, e.g. {:?}",
            missing.len(),
            missing.first()
        );
    }
}
