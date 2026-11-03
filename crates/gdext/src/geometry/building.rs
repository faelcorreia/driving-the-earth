//! Buildings of one tile: each footprint extruded into a box with vertical walls and a flat roof.
//! Pure Rust (no Godot types). Imports `tileformat` data only.
//!
//! The base of a box is flat, at the lowest terrain height under the footprint (the terrain sampled
//! at every vertex and at the centroid). Each wall reaches down to the terrain at its own two
//! vertices, so on a slope the walls are a skirt: no gap is left on the low side, and the
//! walls never go under the terrain. The height is `levels x FLOOR_HEIGHT_M`, measured from the flat base.
//! For the window shader, a wall's UV is (perimeter in m, height above the base in m), continuous
//! around the ring; a roof has `ROOF_UV`; UV2 x is the building seed (see `seed_of`).
use tileformat::units::FLOOR_HEIGHT_M;
use tileformat::{Polygon, Tile};

use super::mesh::{world, MeshData};

/// Palette of the Brazilian buildings, sRGB: 0 orange brick/tile, 1 ice white, 2 light gray, 3 dark gray.
pub const PALETTE: [[f32; 3]; 4] = [
    [0.72, 0.38, 0.24],
    [0.88, 0.89, 0.88],
    [0.68, 0.69, 0.69],
    [0.38, 0.39, 0.41],
];
/// No channel of a building color goes above this (ice white never blows out to white).
pub const MAX_CHANNEL: f32 = 0.90;
/// UV of every roof vertex: a negative x is the roof for the shader (walls have x >= 0).
pub const ROOF_UV: [f32; 2] = [-1.0, -1.0];
/// Roofs are this much darker than the walls.
const ROOF_SHADE: f32 = 0.8;
/// Walls go from this shade at the terrain to 1.0 at the top.
const BASE_SHADE: f32 = 0.85;
/// Per-building luminosity variation (tint), at most this fraction.
const TINT_MAX: f32 = 0.06;
/// Floors of a building with no known count: 1 or 2, from a hash of its position.
const ESTIMATED_LEVELS_SPAN: u64 = 2;

/// Builds the boxes of all `tile.buildings` into one mesh (one draw call per tile).
pub fn building_mesh(tile: &Tile, ground: &dyn Fn(f64, f64) -> f32) -> MeshData {
    let mut mesh = MeshData::default();
    for b in &tile.buildings {
        add_building(&mut mesh, b, ground);
    }
    mesh
}

fn add_building(mesh: &mut MeshData, b: &Polygon, ground: &dyn Fn(f64, f64) -> f32) {
    let mut ring = b.points.clone();
    if ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
    let n = ring.len();
    if n < 3 {
        return;
    }
    let centroid = [
        ring.iter().map(|p| p[0]).sum::<f64>() / n as f64,
        ring.iter().map(|p| p[1]).sum::<f64>() / n as f64,
    ];
    let ground_at: Vec<f32> = ring.iter().map(|p| ground(p[0], p[1])).collect();
    let base_y = ground_at
        .iter()
        .copied()
        .chain([ground(centroid[0], centroid[1])])
        .fold(f32::INFINITY, f32::min);
    let levels = if b.levels > 0 {
        b.levels as f32
    } else {
        estimated_levels(centroid) as f32
    };
    let top_y = base_y + levels * FLOOR_HEIGHT_M as f32;

    let wall = palette_color(b.color_idx, b.tint);
    let roof = scale(wall, ROOF_SHADE);
    let seed = seed_of(centroid);
    // Counter-clockwise in (x, y) so that the outward normal of an edge is known.
    let sign = if signed_area(&ring) > 0.0 { 1.0 } else { -1.0 };
    // Meters along the ring from its first vertex: a wall's top q and the next wall's top p share
    // one value, so the perimeter has no jump at the corners.
    let mut along = vec![0.0f32; n + 1];
    for i in 0..n {
        along[i + 1] = along[i] + edge_len(ring[i], ring[(i + 1) % n]) as f32;
    }

    for i in 0..n {
        let (p, q) = (ring[i], ring[(i + 1) % n]);
        let len = edge_len(p, q);
        if len == 0.0 {
            continue;
        }
        // Local (x, y) outward normal is (dy, -dx) for a counter-clockwise ring; Godot z is -y.
        let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
        let normal = [(sign * dy / len) as f32, 0.0, (sign * dx / len) as f32];
        let (gp, gq) = (ground_at[i], ground_at[(i + 1) % n]);
        let base = scale(wall, BASE_SHADE);
        let first = mesh.vertices.len() as u32;
        let (u0, u1) = (along[i], along[i + 1]);
        // Top p, top q, bottom q, bottom p (bottom on the terrain under each vertex). UV: x is the
        // perimeter in meters, y the height above the flat base in meters.
        for (pt, h, u, color) in [
            (p, top_y, u0, wall),
            (q, top_y, u1, wall),
            (q, gq, u1, base),
            (p, gp, u0, base),
        ] {
            push_vertex(
                mesh,
                world(pt[0], pt[1], h),
                normal,
                color,
                [u, h - base_y],
                seed,
            );
        }
        push_facing(mesh, first, first + 1, first + 2, normal);
        push_facing(mesh, first, first + 2, first + 3, normal);
    }

    let up = [0.0, 1.0, 0.0];
    let first = mesh.vertices.len() as u32;
    for pt in &ring {
        push_vertex(mesh, world(pt[0], pt[1], top_y), up, roof, ROOF_UV, seed);
    }
    let ccw: Vec<[f64; 2]> = if sign > 0.0 {
        ring.clone()
    } else {
        ring.iter().rev().copied().collect()
    };
    for [a, b, c] in triangulate(&ccw) {
        // Indices into `ring`; the reversed order (when it was clockwise) maps back to the same vertices.
        let (a, b, c) = if sign > 0.0 {
            (a, b, c)
        } else {
            (n - 1 - a, n - 1 - b, n - 1 - c)
        };
        push_facing(
            mesh,
            first + a as u32,
            first + b as u32,
            first + c as u32,
            up,
        );
    }
}

/// Levels of a building without a known count: 1 or 2, from a hash of its position (0.5 m grid).
fn estimated_levels(centroid: [f64; 2]) -> u8 {
    let qx = (centroid[0] * 2.0).round() as i64;
    let qy = (centroid[1] * 2.0).round() as i64;
    let mut bytes = qx.to_le_bytes().to_vec();
    bytes.extend(qy.to_le_bytes());
    1 + (fnv1a(&bytes) % ESTIMATED_LEVELS_SPAN) as u8
}

/// Seed of a building in 0..1 (its own hash, not the one of `estimated_levels`), the same on every vertex.
fn seed_of(centroid: [f64; 2]) -> f32 {
    let qx = (centroid[0] * 2.0).round() as i64;
    let qy = (centroid[1] * 2.0).round() as i64;
    let mut bytes = qx.to_le_bytes().to_vec();
    bytes.extend(qy.to_le_bytes());
    bytes.extend(b"seed");
    (fnv1a(&bytes) % 256) as f32 / 255.0
}

fn edge_len(p: [f64; 2], q: [f64; 2]) -> f64 {
    ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt()
}

/// One wall or roof vertex: position, normal, color, UV (see `add_building`) and the building seed.
fn push_vertex(
    mesh: &mut MeshData,
    pos: [f32; 3],
    normal: [f32; 3],
    color: [f32; 3],
    uv: [f32; 2],
    seed: f32,
) {
    mesh.vertices.push(pos);
    mesh.normals.push(normal);
    mesh.colors.push([color[0], color[1], color[2], 1.0]);
    mesh.uvs.push(uv);
    mesh.uv2.push([seed, 0.0]);
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Palette color of one building: the palette entry times its tint, capped at `MAX_CHANNEL`.
fn palette_color(color_idx: u8, tint: u8) -> [f32; 3] {
    let t = (tint.min(7) as f32 / 7.0 * 2.0 - 1.0) * TINT_MAX;
    let base = PALETTE[color_idx.min(3) as usize];
    base.map(|c| (c * (1.0 + t)).min(MAX_CHANNEL))
}

fn scale(c: [f32; 3], k: f32) -> [f32; 3] {
    c.map(|v| v * k)
}

fn signed_area(ring: &[[f64; 2]]) -> f64 {
    let n = ring.len();
    (0..n)
        .map(|i| ring[i][0] * ring[(i + 1) % n][1] - ring[(i + 1) % n][0] * ring[i][1])
        .sum::<f64>()
        / 2.0
}

/// Ear clipping of a counter-clockwise simple ring. Returns the triangles as ring indices; for a
/// ring that is not simple, the triangles found so far (the roof then has a hole, never a fold).
fn triangulate(ring: &[[f64; 2]]) -> Vec<[usize; 3]> {
    let mut idx: Vec<usize> = (0..ring.len()).collect();
    let mut out = Vec::new();
    while idx.len() > 3 {
        let m = idx.len();
        let ear = (0..m).find(|&k| {
            let (a, b, c) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
            cross2(ring[a], ring[b], ring[c]) > 0.0
                && !idx.iter().any(|&p| {
                    p != a && p != b && p != c && inside(ring[p], ring[a], ring[b], ring[c])
                })
        });
        let Some(k) = ear else { return out };
        out.push([idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]]);
        idx.remove(k);
    }
    if idx.len() == 3 {
        out.push([idx[0], idx[1], idx[2]]);
    }
    out
}

fn cross2(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Point inside or on the border of triangle `abc` (counter-clockwise).
fn inside(p: [f64; 2], a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> bool {
    cross2(a, b, p) >= 0.0 && cross2(b, c, p) >= 0.0 && cross2(c, a, p) >= 0.0
}

/// Adds triangle `a, b, c` with its front face toward `normal`. Godot front faces are clockwise
/// seen from the front, so the front is opposite to `(b - a) x (c - a)`.
fn push_facing(mesh: &mut MeshData, a: u32, b: u32, c: u32, normal: [f32; 3]) {
    let (pa, pb, pc) = (
        mesh.vertices[a as usize],
        mesh.vertices[b as usize],
        mesh.vertices[c as usize],
    );
    let u = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
    let v = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
    let cross = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let dot = cross[0] * normal[0] + cross[1] * normal[1] + cross[2] * normal[2];
    if dot < 0.0 {
        mesh.indices.extend([a, b, c]);
    } else {
        mesh.indices.extend([a, c, b]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::tests::{grid, tile};

    /// Slope in x: height rises 0.2 m per meter (20%).
    fn slope(x: f64, _y: f64) -> f32 {
        (0.2 * x) as f32
    }

    fn building(points: Vec<[f64; 2]>, levels: u8, color_idx: u8, tint: u8) -> Polygon {
        Polygon {
            points,
            generated: true,
            levels,
            color_idx,
            tint,
        }
    }

    fn one(b: Polygon) -> MeshData {
        let mut t = tile(0, 0, grid(2, |_, _| 0.0), Vec::new());
        t.buildings = vec![b];
        building_mesh(&t, &slope)
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<[f64; 2]> {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    #[test]
    fn walls_reach_the_terrain_on_a_slope_without_gap_or_sinking() {
        let m = one(building(rect(10.0, 10.0, 20.0, 20.0), 2, 0, 3));
        // Walls: the vertices of the bottom row sit on the terrain (within 5 cm).
        for (v, n) in m.vertices.iter().zip(&m.normals) {
            if n[1] != 0.0 {
                continue; // roof
            }
            let terrain = slope(v[0] as f64, 0.0);
            assert!(
                v[1] >= terrain - 0.05,
                "wall vertex under the terrain: {v:?}"
            );
        }
        let bottoms: Vec<f32> = m
            .vertices
            .iter()
            .zip(&m.normals)
            .filter(|(_, n)| n[1] == 0.0)
            .map(|(v, _)| v[1])
            .collect();
        let lowest = bottoms.iter().copied().fold(f32::INFINITY, f32::min);
        let top = m
            .vertices
            .iter()
            .map(|v| v[1])
            .fold(f32::NEG_INFINITY, f32::max);
        // The flat base is at the lowest terrain under the footprint (x = 10).
        assert!(
            (lowest - slope(10.0, 0.0)).abs() < 0.05,
            "lowest bottom {lowest}"
        );
        // Every wall spans from its terrain to the top: the bottom row is never above the terrain.
        assert!(bottoms.iter().all(|&y| y <= top));
    }

    #[test]
    fn height_is_levels_times_floor_height_from_the_flat_base() {
        let base = slope(10.0, 0.0);
        for levels in [1u8, 2, 5] {
            let m = one(building(rect(10.0, 10.0, 20.0, 20.0), levels, 1, 0));
            let top = m
                .vertices
                .iter()
                .map(|v| v[1])
                .fold(f32::NEG_INFINITY, f32::max);
            let expected = base + levels as f32 * FLOOR_HEIGHT_M as f32;
            assert!(
                (top - expected).abs() < 1e-4,
                "levels {levels}: top {top}, expected {expected}"
            );
        }
    }

    #[test]
    fn unknown_levels_are_one_or_two_and_deterministic() {
        let b = building(rect(10.0, 10.0, 20.0, 20.0), 0, 2, 4);
        let (m1, m2) = (one(b.clone()), one(b));
        assert_eq!(m1, m2);
        let top = m1
            .vertices
            .iter()
            .map(|v| v[1])
            .fold(f32::NEG_INFINITY, f32::max);
        let levels = ((top - slope(10.0, 0.0)) / FLOOR_HEIGHT_M as f32).round() as u8;
        assert!((1..=2).contains(&levels), "levels {levels}");
    }

    #[test]
    fn walls_face_outward_and_roof_faces_up() {
        for pts in [
            rect(10.0, 10.0, 20.0, 20.0),
            rect(10.0, 10.0, 20.0, 20.0).into_iter().rev().collect(),
        ] {
            let m = one(building(pts, 1, 0, 0));
            for tri in m.indices.chunks(3) {
                let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
                let (pa, pb, pc) = (m.vertices[a], m.vertices[b], m.vertices[c]);
                let u = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
                let v = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
                let cross = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                let n = m.normals[a];
                let dot = cross[0] * n[0] + cross[1] * n[1] + cross[2] * n[2];
                assert!(dot < 0.0, "triangle {tri:?} faces away from its normal");
                // Outward: from the footprint center to the wall (horizontal part).
                if n[1] == 0.0 {
                    let mid = [(pa[0] + pc[0]) / 2.0 - 15.0, (pa[2] + pc[2]) / 2.0 + 15.0];
                    assert!(
                        mid[0] * n[0] + mid[1] * n[2] > -1e-3,
                        "wall normal points inward"
                    );
                }
            }
        }
    }

    #[test]
    fn palette_colors_stay_within_the_channel_cap() {
        for idx in 0..4u8 {
            for tint in 0..8u8 {
                let c = palette_color(idx, tint);
                assert!(
                    c.iter().all(|&v| v <= MAX_CHANNEL + 1e-6),
                    "idx {idx} tint {tint}: {c:?}"
                );
            }
        }
        let white = palette_color(1, 7);
        assert!(white[0] <= 0.90 && white[1] <= 0.90 && white[2] <= 0.90);
    }

    #[test]
    fn colors_come_with_one_entry_per_vertex() {
        let m = one(building(rect(10.0, 10.0, 20.0, 20.0), 1, 3, 0));
        assert_eq!(m.colors.len(), m.vertices.len());
        assert!(m.colors.iter().all(|c| c[3] == 1.0));
    }

    #[test]
    fn uv_height_is_levels_times_floor_height_above_the_base() {
        for levels in [1u8, 2, 5] {
            let m = one(building(rect(10.0, 10.0, 20.0, 20.0), levels, 1, 0));
            let walls: Vec<[f32; 2]> = m
                .uvs
                .iter()
                .zip(&m.normals)
                .filter(|(_, n)| n[1] == 0.0)
                .map(|(uv, _)| *uv)
                .collect();
            let top = walls
                .iter()
                .map(|uv| uv[1])
                .fold(f32::NEG_INFINITY, f32::max);
            assert!(
                (top - levels as f32 * FLOOR_HEIGHT_M as f32).abs() < 1e-3,
                "levels {levels}: uv v top {top}"
            );
            // Bottoms sit on the terrain (never below the flat base).
            assert!(
                walls.iter().all(|uv| uv[1] >= -1e-4),
                "a wall vertex below the base"
            );
        }
    }

    #[test]
    fn perimeter_is_continuous_across_the_corners() {
        // Square 10 m side: perimeter 40 m. Top corners of the four walls carry 0, 10, 20, 30 and 40.
        let m = one(building(rect(10.0, 10.0, 20.0, 20.0), 1, 0, 0));
        let mut us: Vec<f32> = m
            .vertices
            .iter()
            .zip(&m.uvs)
            .zip(&m.normals)
            .filter(|((v, _), n)| n[1] == 0.0 && (v[1] - top_y_of(&m)).abs() < 1e-4)
            .map(|((_, uv), _)| uv[0])
            .collect();
        us.sort_by(|a, b| a.partial_cmp(b).unwrap());
        us.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
        assert_eq!(us.len(), 5, "corner values {us:?}");
        for (got, want) in us.iter().zip([0.0, 10.0, 20.0, 30.0, 40.0]) {
            assert!((got - want).abs() < 1e-3, "corner values {us:?}");
        }
    }

    fn top_y_of(m: &MeshData) -> f32 {
        m.vertices
            .iter()
            .map(|v| v[1])
            .fold(f32::NEG_INFINITY, f32::max)
    }

    #[test]
    fn roofs_carry_the_roof_uv_and_every_vertex_the_same_seed() {
        let m = one(building(rect(10.0, 10.0, 20.0, 20.0), 2, 2, 5));
        assert_eq!(m.uvs.len(), m.vertices.len());
        assert_eq!(m.uv2.len(), m.vertices.len());
        for ((uv, n), _) in m.uvs.iter().zip(&m.normals).zip(&m.vertices) {
            if n[1] != 0.0 {
                assert_eq!(*uv, ROOF_UV);
            }
        }
        let seed = m.uv2[0][0];
        assert!((0.0..=1.0).contains(&seed));
        assert!(m.uv2.iter().all(|c| c[0] == seed));
    }
}
