//! Road geometry: ribbons that follow the terrain, and the lowering of terrain under roads.
//! Pure Rust (no Godot types). Imports `geometry::{mesh, vec2}` and `tileformat` data.
use tileformat::{road_width_m, Polyline};

use super::mesh::{push_up_facing, world, MeshData};
use super::vec2::{dist, miter, scale, unit_normal};

/// Height of the road ribbons above the terrain, in meters (avoids z-fighting).
pub(crate) const ROAD_LIFT_M: f32 = 0.1;
/// Extra lift per road index (modulo 8), so overlapping ribbons do not z-fight.
pub(super) const ROAD_ORDER_LIFT_M: f32 = 0.002;

/// Terrain vertices closer than this (in grid cells, beyond the half width) to a road with a
/// profile are lowered. Any triangle that touches the band then has all its corners lowered.
const LOWER_MARGIN_CELLS: f64 = 1.5;
/// Lowered terrain vertices sit this far below the road axis height (the road lies above).
const LOWER_BELOW_AXIS_M: f32 = 0.1;

/// Terrain grid heights (row-major, `n` per side, origin `origin` in map meters) with each
/// vertex near a road that has a profile lowered to `z - LOWER_BELOW_AXIS_M`, where `z` is
/// the height of the nearest axis point. Vertices never go up, and the triangles stay as
/// they are: the road ribbon then lies above the terrain where the grid resolves a crest.
pub(super) fn lower_under_roads(
    values: &[f32],
    n: usize,
    origin: [f64; 2],
    cell: f64,
    roads: &[Polyline],
    ends: &[(Vec<[f64; 2]>, Vec<f32>)],
) -> Vec<f32> {
    let mut out = values.to_vec();
    for (road, (pts, zs)) in roads.iter().zip(ends) {
        if pts.len() < 2 || zs.len() != pts.len() {
            continue;
        }
        let reach = road_width_m(&road.highway) / 2.0 + LOWER_MARGIN_CELLS * cell;
        for k in 0..pts.len() - 1 {
            let (a, b) = (pts[k], pts[k + 1]);
            let (za, zb) = (zs[k], zs[k + 1]);
            let d = [b[0] - a[0], b[1] - a[1]];
            let len2 = d[0] * d[0] + d[1] * d[1];
            // Index range of the grid vertices in the segment's box, grown by `reach`.
            let range = |lo: f64, hi: f64, o: f64| {
                let i0 = ((lo - reach - o) / cell).floor().max(0.0) as usize;
                let i1 = (((hi + reach - o) / cell).ceil().max(0.0) as usize).min(n - 1);
                (i0, i1)
            };
            let (j0, j1) = range(a[0].min(b[0]), a[0].max(b[0]), origin[0]);
            let (i0, i1) = range(a[1].min(b[1]), a[1].max(b[1]), origin[1]);
            for i in i0..=i1 {
                for j in j0..=j1 {
                    let p = [origin[0] + j as f64 * cell, origin[1] + i as f64 * cell];
                    let t = if len2 > 0.0 {
                        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len2).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let q = [a[0] + d[0] * t, a[1] + d[1] * t];
                    if (p[0] - q[0]).hypot(p[1] - q[1]) > reach {
                        continue;
                    }
                    let cand = za + (zb - za) * t as f32 - LOWER_BELOW_AXIS_M;
                    let v = &mut out[i * n + j];
                    if cand < *v {
                        *v = cand;
                    }
                }
            }
        }
    }
    out
}

/// Stations per terrain grid cell along a road.
pub(super) const ROAD_STATIONS_PER_CELL: f64 = 4.0;

/// Adds one road as a continuous strip: three vertices per station (left edge, center,
/// right edge), two quads between stations. Stations sit at the polyline nodes and every
/// `step` meters in between, so each vertex is sampled on the terrain and the strip
/// does not cut under a crest between two nodes. At each node the two segment normals
/// are joined (miter), so bends have no gaps or overlaps.
pub(super) fn push_road(mesh: &mut MeshData, ground: &dyn Fn(f64, f64) -> f32, points: &[[f64; 2]], zs: &[f32], half: f64, step: f64, lift: f32) {
    // Drop repeated points (and their heights): they have no direction.
    let mut pts: Vec<[f64; 2]> = Vec::with_capacity(points.len());
    let mut heights: Vec<f32> = Vec::with_capacity(zs.len());
    for (k, &p) in points.iter().enumerate() {
        match pts.last() {
            Some(&q) if dist(p, q) <= 1e-9 => {}
            _ => {
                pts.push(p);
                if let Some(&z) = zs.get(k) {
                    heights.push(z);
                }
            }
        }
    }
    if pts.len() < 2 {
        return;
    }
    let has_z = heights.len() == pts.len();
    let normals: Vec<[f64; 2]> = pts.windows(2).map(|w| unit_normal(w[0], w[1])).collect();

    // Offset from the center to the left edge, one per station, with its position and,
    // when the bake gave a profile, the height of the axis there.
    let mut stations: Vec<([f64; 2], [f64; 2], Option<f32>)> = Vec::new();
    stations.push((pts[0], scale(normals[0], half), has_z.then(|| heights[0])));
    for k in 0..pts.len() - 1 {
        let (p, q) = (pts[k], pts[k + 1]);
        let pieces = (dist(p, q) / step).ceil().max(1.0) as usize;
        for s in 1..pieces {
            let t = s as f64 / pieces as f64;
            let at = [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t];
            let z = has_z.then(|| heights[k] + (heights[k + 1] - heights[k]) * t as f32);
            stations.push((at, scale(normals[k], half), z));
        }
        let offset = match normals.get(k + 1) {
            Some(&next) => miter(normals[k], next, half),
            None => scale(normals[k], half),
        };
        stations.push((q, offset, has_z.then(|| heights[k + 1])));
    }

    let base = mesh.vertices.len() as u32;
    for &(c, o, z) in &stations {
        for [x, y] in [[c[0] + o[0], c[1] + o[1]], c, [c[0] - o[0], c[1] - o[1]]] {
            let height = match z {
                Some(z) => z + lift,
                None => ground(x, y) + lift,
            };
            mesh.vertices.push(world(x, y, height));
            mesh.normals.push([0.0, 1.0, 0.0]);
        }
    }
    for s in 0..stations.len() - 1 {
        // Vertex of station s on the left (3s), center (3s + 1) and right (3s + 2) edge.
        let l0 = base + 3 * s as u32;
        let l1 = l0 + 3;
        push_up_facing(mesh, l0, l0 + 1, l1 + 1);
        push_up_facing(mesh, l0, l1 + 1, l1);
        push_up_facing(mesh, l0 + 1, l0 + 2, l1 + 2);
        push_up_facing(mesh, l0 + 1, l1 + 2, l1 + 1);
    }
}
