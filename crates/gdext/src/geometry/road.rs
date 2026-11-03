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

/// Adds one road as a continuous strip: three vertices per station (left edge, center,
/// right edge), two quads between stations. Stations sit at the polyline nodes and every
/// `ROAD_STATION_M` meters in between. Every vertex takes the height of the terrain field at
/// its own position plus `lift`, so the ribbon lies on the same surface as the terrain mesh.
/// At each node the two segment normals are joined (miter), so bends have no gaps or overlaps.
pub(super) fn push_road(
    mesh: &mut MeshData,
    surface: &Surface,
    ground: &dyn Fn(f64, f64) -> f32,
    points: &[[f64; 2]],
    half: f64,
    lift: f32,
) {
    // Drop repeated points: they have no direction.
    let mut pts: Vec<[f64; 2]> = Vec::with_capacity(points.len());
    for &p in points {
        match pts.last() {
            Some(&q) if dist(p, q) <= 1e-9 => {}
            _ => pts.push(p),
        }
    }
    if pts.len() < 2 {
        return;
    }
    let normals: Vec<[f64; 2]> = pts.windows(2).map(|w| unit_normal(w[0], w[1])).collect();

    // Offset from the center to the left edge, one per station, and the station position.
    let mut stations: Vec<([f64; 2], [f64; 2])> = Vec::new();
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
        stations.push((q, offset));
    }

    let base = mesh.vertices.len() as u32;
    for &(c, o) in &stations {
        for [x, y] in [[c[0] + o[0], c[1] + o[1]], c, [c[0] - o[0], c[1] - o[1]]] {
            let height = surface.height(ground, x, y) + lift;
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
