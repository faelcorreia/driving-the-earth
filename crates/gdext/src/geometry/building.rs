//! Buildings of one tile: each footprint extruded into a box with vertical walls and a flat roof.
//! Pure Rust (no Godot types). Imports `tileformat` data and the active `style`.
//!
//! The base of a box is flat, at the lowest terrain height under the footprint (the terrain sampled
//! at every vertex and at the centroid). Each wall reaches down to the terrain at its own two
//! vertices, so on a slope the walls are a skirt: no gap is left on the low side, and the
//! walls never go under the terrain. The height is `levels x FLOOR_HEIGHT_M`, measured from the flat base.
//! For the window shader, a wall's UV is (perimeter in m, height above the base in m), continuous
//! around the ring; a roof has `ROOF_UV`; UV2 x is the building seed (see `seed_of`) and UV2 y the
//! height of the vertex above the terrain under it (a wall part under 1 m of ground gets no window).
//!
//! Every vertical corner of the footprint is cut back by `world.bevel_m` (chamfered, or rounded
//! when `bevel_segments > 1`) before the walls and the roof are built; the collision prism keeps
//! the original footprint hull, so the car physics does not change.
use tileformat::units::FLOOR_HEIGHT_M;
use tileformat::{Polygon, Tile};

use super::mesh::{normalize, world, MeshData};
use crate::style::{self, Rgb, WorldLook};

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
/// A corner is cut back by at most this share of its shortest adjacent edge, so a small footprint
/// never loses a corner entirely.
const BEVEL_CORNER_FRACTION: f64 = 0.25;
/// Two cutbacks on the same edge leave at least this share of it as a wall, so they never meet.
const BEVEL_EDGE_FRACTION: f64 = 0.8;
/// Cutbacks below this (in m) are dropped: they would only add degenerate, zero-area faces.
const BEVEL_MIN_M: f64 = 1e-6;

/// Builds the boxes of all `tile.buildings` into one mesh (one draw call per tile), and the convex
/// prism of each building whose footprint has area (see `BuildingPrism`), with the active theme
/// bevel, flat shading and facade colors.
pub fn building_mesh(
    tile: &Tile,
    ground: &dyn Fn(f64, f64) -> f32,
) -> (MeshData, Vec<BuildingPrism>) {
    let mut look = style::world_look();
    // The four facade/roof colors are the theme's `world.palette.buildings`.
    look.palette.buildings = style::building_rgb();
    building_mesh_with(tile, ground, look)
}

/// `building_mesh` with an explicit look, so the tests do not depend on the installed theme.
fn building_mesh_with(
    tile: &Tile,
    ground: &dyn Fn(f64, f64) -> f32,
    look: WorldLook,
) -> (MeshData, Vec<BuildingPrism>) {
    let mut mesh = MeshData::default();
    let mut prisms = Vec::new();
    for b in &tile.buildings {
        add_building(&mut mesh, &mut prisms, b, ground, &look);
    }
    (mesh, prisms)
}

fn add_building(
    mesh: &mut MeshData,
    prisms: &mut Vec<BuildingPrism>,
    b: &Polygon,
    ground: &dyn Fn(f64, f64) -> f32,
    look: &WorldLook,
) {
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
    // Collision is the convex prism of the original footprint, never of the rounded ring.
    if let Some(prism) = prism_of(&ring, base_y, top_y) {
        prisms.push(prism);
    }

    let wall = palette_color(&look.palette.buildings, b.color_idx, b.tint);
    let roof = scale(wall, ROOF_SHADE);
    let seed = seed_of(centroid);
    // Counter-clockwise in (x, y) so that the outward normal of an edge is known.
    let sign = if signed_area(&ring) > 0.0 { 1.0 } else { -1.0 };
    let rounded = rounded_ring(&ring, look.bevel_m, look.bevel_segments);
    let points = &rounded.points;
    let m = points.len();
    if m < 3 {
        return;
    }
    let ground_pts: Vec<f32> = points.iter().map(|p| ground(p[0], p[1])).collect();
    // Meters along the rounded ring from its first vertex: a wall's top q and the next wall's top
    // p share one value, so the perimeter (and the window grid) has no jump at a corner or chamfer.
    let mut along = vec![0.0f32; m + 1];
    for j in 0..m {
        along[j + 1] = along[j] + edge_len(points[j], points[(j + 1) % m]) as f32;
    }
    let face = wall_face_normals(points, sign);
    let smooth = smoothed_corner_normals(&face, &rounded.chamfer);

    for j in 0..m {
        let k = (j + 1) % m;
        let (p, q) = (points[j], points[k]);
        let len = edge_len(p, q);
        if len == 0.0 {
            continue;
        }
        let normal = face[j];
        // `flat_shading` keeps one normal per face; otherwise only the rounded corner segments
        // average the normals of their neighbours (the flat walls keep their own face normal).
        let (np, nq) = if !look.flat_shading && rounded.chamfer[j] {
            (smooth[j], smooth[k])
        } else {
            (normal, normal)
        };
        let (gp, gq) = (ground_pts[j], ground_pts[k]);
        let base = scale(wall, BASE_SHADE);
        let first = mesh.vertices.len() as u32;
        let (u0, u1) = (along[j], along[j + 1]);
        // Top p, top q, bottom q, bottom p (bottom on the terrain under each vertex). UV: x is the
        // perimeter in meters, y the height above the flat base in meters. UV2.y is the height of
        // the vertex above the terrain under it (the window mask only applies above 1 m).
        for (pt, h, g, u, color, nrm) in [
            (p, top_y, gp, u0, wall, np),
            (q, top_y, gq, u1, wall, nq),
            (q, gq, gq, u1, base, nq),
            (p, gp, gp, u0, base, np),
        ] {
            push_vertex(
                mesh,
                world(pt[0], pt[1], h),
                nrm,
                color,
                [u, h - base_y],
                [seed, (h - g).max(0.0)],
            );
        }
        push_facing(mesh, first, first + 1, first + 2, normal);
        push_facing(mesh, first, first + 2, first + 3, normal);
    }

    let up = [0.0, 1.0, 0.0];
    let first = mesh.vertices.len() as u32;
    for pt in points {
        push_vertex(
            mesh,
            world(pt[0], pt[1], top_y),
            up,
            roof,
            ROOF_UV,
            [seed, 0.0],
        );
    }
    let ccw: Vec<[f64; 2]> = if sign > 0.0 {
        points.clone()
    } else {
        points.iter().rev().copied().collect()
    };
    for [a, b, c] in triangulate(&ccw) {
        // Indices into `points`; the reversed order (when it was clockwise) maps back to the same vertices.
        let (a, b, c) = if sign > 0.0 {
            (a, b, c)
        } else {
            (m - 1 - a, m - 1 - b, m - 1 - c)
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

/// Footprint ring with every corner cut back by `bevel_m`: the chamfer starts and ends `d` meters
/// from the corner along its two edges, and a quadratic curve with the original corner as control
/// point is sampled into `bevel_segments` pieces (a single piece is the straight chamfer). The ring
/// keeps the winding of the original footprint.
struct RoundedRing {
    points: Vec<[f64; 2]>,
    /// Whether the segment `j -> j + 1` belongs to a chamfer (both points come from one corner).
    chamfer: Vec<bool>,
}

fn rounded_ring(ring: &[[f64; 2]], bevel_m: f64, segments: u8) -> RoundedRing {
    let n = ring.len();
    let cut = cutbacks(ring, bevel_m);
    let pieces = segments.clamp(1, 3) as usize;
    let mut points = Vec::with_capacity(n * (pieces + 1));
    let mut corner = Vec::with_capacity(points.capacity());
    for i in 0..n {
        if cut[i] <= 0.0 {
            points.push(ring[i]);
            corner.push(i);
            continue;
        }
        let prev = ring[(i + n - 1) % n];
        let next = ring[(i + 1) % n];
        let a = towards(ring[i], prev, cut[i]);
        let b = towards(ring[i], next, cut[i]);
        for k in 0..=pieces {
            let t = k as f64 / pieces as f64;
            points.push(quadratic(a, ring[i], b, t));
            corner.push(i);
        }
    }
    let m = points.len();
    let chamfer = (0..m).map(|j| corner[j] == corner[(j + 1) % m]).collect();
    RoundedRing { points, chamfer }
}

/// Cutback distance of each corner, in meters: at most `bevel_m`, at most a quarter of the
/// shortest adjacent edge, and then reduced along each edge until the two cutbacks on it leave a
/// wall (no crossing, even on a small or thin footprint).
fn cutbacks(ring: &[[f64; 2]], bevel_m: f64) -> Vec<f64> {
    let n = ring.len();
    let mut d = vec![0.0; n];
    if bevel_m <= 0.0 || n < 3 {
        return d;
    }
    for i in 0..n {
        let prev = edge_len(ring[(i + n - 1) % n], ring[i]);
        let next = edge_len(ring[i], ring[(i + 1) % n]);
        d[i] = bevel_m.min(BEVEL_CORNER_FRACTION * prev.min(next));
    }
    // A reduction only ever lowers a sum, so one pass leaves every edge satisfied (a second pass
    // covers the wrap-around in the same way).
    for _ in 0..2 {
        for i in 0..n {
            let k = (i + 1) % n;
            let sum = d[i] + d[k];
            let limit = BEVEL_EDGE_FRACTION * edge_len(ring[i], ring[k]);
            if sum > limit && sum > 0.0 {
                let scale = limit / sum;
                d[i] *= scale;
                d[k] *= scale;
            }
        }
    }
    for value in &mut d {
        if *value < BEVEL_MIN_M {
            *value = 0.0;
        }
    }
    d
}

/// Point `dist` meters from `from` toward `to` (`dist` must not exceed the edge length).
fn towards(from: [f64; 2], to: [f64; 2], dist: f64) -> [f64; 2] {
    let len = edge_len(from, to);
    if len == 0.0 {
        return from;
    }
    let k = dist / len;
    [
        from[0] + (to[0] - from[0]) * k,
        from[1] + (to[1] - from[1]) * k,
    ]
}

/// Quadratic Bezier point at `t`, from `a` to `b` with `control` as the original corner.
fn quadratic(a: [f64; 2], control: [f64; 2], b: [f64; 2], t: f64) -> [f64; 2] {
    let u = 1.0 - t;
    [
        u * u * a[0] + 2.0 * u * t * control[0] + t * t * b[0],
        u * u * a[1] + 2.0 * u * t * control[1] + t * t * b[1],
    ]
}

/// Outward horizontal normal of every segment of the rounded ring (`sign` is +1 for a
/// counter-clockwise footprint, as in `add_building`).
fn wall_face_normals(points: &[[f64; 2]], sign: f64) -> Vec<[f32; 3]> {
    let m = points.len();
    (0..m)
        .map(|j| {
            let (p, q) = (points[j], points[(j + 1) % m]);
            let len = edge_len(p, q);
            if len == 0.0 {
                return [0.0; 3];
            }
            let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
            [(sign * dy / len) as f32, 0.0, (sign * dx / len) as f32]
        })
        .collect()
}

/// Per-vertex normal of the rounded corners: the average of the chamfer faces that meet at the
/// point, so a rounded corner shades smoothly while every flat wall keeps its single face normal.
fn smoothed_corner_normals(face: &[[f32; 3]], chamfer: &[bool]) -> Vec<[f32; 3]> {
    let m = face.len();
    let mut acc = vec![[0.0f64; 3]; m];
    for j in 0..m {
        if !chamfer[j] {
            continue;
        }
        let k = (j + 1) % m;
        for i in [j, k] {
            acc[i][0] += face[j][0] as f64;
            acc[i][1] += face[j][1] as f64;
            acc[i][2] += face[j][2] as f64;
        }
    }
    acc.iter()
        .enumerate()
        .map(|(i, a)| {
            if *a == [0.0; 3] {
                face[i]
            } else {
                normalize(*a)
            }
        })
        .collect()
}

/// Convex prism of one building: the convex hull of its footprint, from the flat base (the seat)
/// up to the roof height. `points` are the hull corners at the base, then the same at the top, in
/// Godot world space, so they feed a `ConvexPolygonShape3D` as they are.
#[derive(Clone, Debug, PartialEq)]
pub struct BuildingPrism {
    pub points: Vec<[f32; 3]>,
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

/// Convex prism of a footprint ring at heights `base_y` and `top_y`; `None` when its hull has
/// fewer than three corners (no area: no prism, the wall mesh alone stays).
fn prism_of(ring: &[[f64; 2]], base_y: f32, top_y: f32) -> Option<BuildingPrism> {
    let hull = convex_hull(ring);
    if hull.len() < 3 {
        return None;
    }
    let bottom = hull.iter().map(|p| world(p[0], p[1], base_y));
    let top = hull.iter().map(|p| world(p[0], p[1], top_y));
    Some(BuildingPrism {
        points: bottom.chain(top).collect(),
    })
}

/// Convex hull of points (Andrew's monotone chain), counter-clockwise, without collinear corners.
fn convex_hull(points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut pts = points.to_vec();
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let mut hull: Vec<[f64; 2]> = Vec::with_capacity(2 * pts.len());
    for pass in [pts.iter().collect::<Vec<_>>(), pts.iter().rev().collect()] {
        let start = hull.len();
        for &p in pass {
            while hull.len() >= start + 2
                && cross2(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0
            {
                hull.pop();
            }
            hull.push(p);
        }
        // The last point of each pass is the first point of the next one.
        hull.pop();
    }
    hull
}

fn edge_len(p: [f64; 2], q: [f64; 2]) -> f64 {
    ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt()
}

/// One wall or roof vertex: position, normal, color, UV (see `add_building`) and UV2 (the building
/// seed in x, the height above the terrain in y).
fn push_vertex(
    mesh: &mut MeshData,
    pos: [f32; 3],
    normal: [f32; 3],
    color: [f32; 3],
    uv: [f32; 2],
    uv2: [f32; 2],
) {
    mesh.vertices.push(pos);
    mesh.normals.push(normal);
    mesh.colors.push([color[0], color[1], color[2], 1.0]);
    mesh.uvs.push(uv);
    mesh.uv2.push(uv2);
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Palette color of one building: the theme's facade entry times its tint, capped at `MAX_CHANNEL`.
fn palette_color(palette: &[Rgb; 4], color_idx: u8, tint: u8) -> [f32; 3] {
    let t = (tint.min(7) as f32 / 7.0 * 2.0 - 1.0) * TINT_MAX;
    let base = palette[color_idx.min(3) as usize];
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

    /// Look of the shipped `lowpoly` theme: bevel 0.22 m, one segment, flat shading.
    fn rounded_look() -> WorldLook {
        WorldLook {
            bevel_m: 0.22,
            bevel_segments: 1,
            flat_shading: true,
            ..WorldLook::default()
        }
    }

    /// No bevel: the footprint exactly as before item 119.
    fn sharp_look() -> WorldLook {
        WorldLook {
            bevel_m: 0.0,
            bevel_segments: 1,
            flat_shading: true,
            ..WorldLook::default()
        }
    }

    fn one(b: Polygon) -> MeshData {
        one_with_prisms(b).0
    }

    fn one_with_prisms(b: Polygon) -> (MeshData, Vec<BuildingPrism>) {
        one_with(b, rounded_look())
    }

    fn one_with(b: Polygon, look: WorldLook) -> (MeshData, Vec<BuildingPrism>) {
        let mut t = tile(0, 0, grid(2, |_, _| 0.0), Vec::new());
        t.buildings = vec![b];
        building_mesh_with(&t, &slope, look)
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<[f64; 2]> {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    /// Footprint with a notch: not convex, so its prism must be the hull, not the footprint.
    fn l_shape() -> Vec<[f64; 2]> {
        vec![
            [10.0, 10.0],
            [20.0, 10.0],
            [20.0, 13.0],
            [13.0, 13.0],
            [13.0, 20.0],
            [10.0, 20.0],
        ]
    }

    /// 2D corners of a prism (its base row), in the order of `convex_hull`.
    fn base_corners(p: &BuildingPrism, base: f32) -> Vec<[f64; 2]> {
        p.points
            .iter()
            .filter(|v| (v[1] - base).abs() < 1e-4)
            .map(|v| [v[0] as f64, -(v[2] as f64)])
            .collect()
    }

    #[test]
    fn prism_contains_the_footprint_and_is_convex() {
        for pts in [rect(10.0, 10.0, 20.0, 20.0), l_shape()] {
            let (_, prisms) = one_with_prisms(building(pts.clone(), 2, 0, 0));
            assert_eq!(prisms.len(), 1);
            let hull = base_corners(&prisms[0], slope(10.0, 0.0));
            // Convex: every corner turns left (counter-clockwise, no collinear corner).
            for i in 0..hull.len() {
                let (a, b, c) = (
                    hull[i],
                    hull[(i + 1) % hull.len()],
                    hull[(i + 2) % hull.len()],
                );
                assert!(
                    cross2(a, b, c) > 0.0,
                    "hull corner {i} not convex: {hull:?}"
                );
            }
            // Contains: every footprint vertex lies inside or on the hull.
            for p in &pts {
                for i in 0..hull.len() {
                    let (a, b) = (hull[i], hull[(i + 1) % hull.len()]);
                    assert!(
                        cross2(a, b, *p) >= -1e-9,
                        "footprint point {p:?} outside hull"
                    );
                }
            }
        }
    }

    #[test]
    fn prism_spans_from_the_seat_to_the_roof() {
        let base = slope(10.0, 0.0);
        let levels = 3u8;
        let (_, prisms) = one_with_prisms(building(l_shape(), levels, 0, 0));
        let p = &prisms[0];
        let lowest = p.points.iter().map(|v| v[1]).fold(f32::INFINITY, f32::min);
        let highest = p
            .points
            .iter()
            .map(|v| v[1])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            (lowest - base).abs() < 1e-4,
            "seat {lowest}, expected {base}"
        );
        let expected_top = base + levels as f32 * FLOOR_HEIGHT_M as f32;
        assert!(
            (highest - expected_top).abs() < 1e-4,
            "roof {highest}, expected {expected_top}"
        );
        // Same corners at the seat and at the roof: 2 x hull corners.
        assert_eq!(p.points.len() % 2, 0);
        assert_eq!(base_corners(p, base).len(), p.points.len() / 2);
    }

    #[test]
    fn a_building_without_area_has_no_prism() {
        let line = vec![[10.0, 10.0], [15.0, 10.0], [20.0, 10.0]];
        // The visible walls are unchanged by the prism (they are drawn as before); only no prism.
        let (_, prisms) = one_with_prisms(building(line, 1, 0, 0));
        assert!(prisms.is_empty());
    }

    #[test]
    fn convex_hull_drops_collinear_and_duplicate_corners() {
        let ring = vec![
            [0.0, 0.0],
            [1.0, 0.0],
            [2.0, 0.0],
            [2.0, 2.0],
            [0.0, 2.0],
            [1.0, 2.0],
            [0.0, 0.0],
        ];
        assert_eq!(convex_hull(&ring).len(), 4);
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
    fn palette_colors_come_from_the_theme_and_stay_within_the_channel_cap() {
        let palette = WorldLook::default().palette.buildings;
        for idx in 0..4u8 {
            for tint in 0..8u8 {
                let c = palette_color(&palette, idx, tint);
                assert!(
                    c.iter().all(|&v| v <= MAX_CHANNEL + 1e-6),
                    "idx {idx} tint {tint}: {c:?}"
                );
            }
        }
        // Each facade slot keeps the hue of its theme color (the tint only scales it).
        let bright = [0.5f32, 0.25, 0.75];
        for idx in 0..4u8 {
            let c = palette_color(&[bright; 4], idx, 7);
            assert!((c[0] - 0.5 * (1.0 + TINT_MAX)).abs() < 1e-6);
            assert!((c[1] - 0.25 * (1.0 + TINT_MAX)).abs() < 1e-6);
            assert!((c[2] - 0.75 * (1.0 + TINT_MAX)).abs() < 1e-6);
        }
        let white = palette_color(&[[1.0; 3]; 4], 1, 7);
        assert!(white[0] <= MAX_CHANNEL && white[1] <= MAX_CHANNEL && white[2] <= MAX_CHANNEL);
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
    fn perimeter_is_continuous_across_the_sharp_corners() {
        // No bevel, square 10 m side: perimeter 40 m. Top corners carry 0, 10, 20, 30 and 40.
        let m = one_with(
            building(rect(10.0, 10.0, 20.0, 20.0), 1, 0, 0),
            sharp_look(),
        )
        .0;
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

    #[test]
    fn perimeter_is_continuous_through_a_rounded_corner() {
        // Every wall quad is pushed as (top p, top q, bottom q, bottom p): the u of each segment
        // grows by its own edge length, and a segment starts with the u the previous one ended.
        let m = one(building(rect(10.0, 10.0, 20.0, 20.0), 1, 0, 0));
        let walls: Vec<usize> = (0..m.vertices.len())
            .filter(|&i| m.normals[i][1] == 0.0)
            .collect();
        assert_eq!(walls.len() % 4, 0);
        let groups = walls.len() / 4;
        assert_eq!(groups, 8, "one chamfer segment per corner, plus four walls");
        let top = top_y_of(&m);
        assert!(m.uvs[walls[0]][0].abs() < 1e-4, "the ring starts at 0 m");
        for g in 0..groups {
            let a = walls[4 * g];
            let b = walls[4 * g + 1];
            let (pa, pb) = (m.vertices[a], m.vertices[b]);
            assert!((pa[1] - top).abs() < 1e-4 && (pb[1] - top).abs() < 1e-4);
            let (ua, ub) = (m.uvs[a][0], m.uvs[b][0]);
            let len = ((pb[0] - pa[0]).powi(2) + (pb[2] - pa[2]).powi(2)).sqrt();
            assert!(
                (ub - ua - len).abs() < 1e-3,
                "u jump on segment {g}: {ua} -> {ub}, edge {len} m"
            );
            if g + 1 == groups {
                // The ring closes here: u runs 0 -> perimeter once, so the last edge ends at the
                // perimeter and the first vertex keeps 0 (the single seam, as before item 119).
                assert!(ub > 1.0, "the perimeter did not close");
                continue;
            }
            let next = walls[4 * (g + 1)];
            let pn = m.vertices[next];
            assert!(
                (pn[0] - pb[0]).abs() < 1e-3 && (pn[2] - pb[2]).abs() < 1e-3,
                "segment {g} does not end where the next one starts"
            );
            assert!(
                (m.uvs[next][0] - ub).abs() < 1e-3,
                "u jump at a ring vertex: {ub} -> {}",
                m.uvs[next][0]
            );
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

    #[test]
    fn a_building_on_a_slope_is_at_least_one_floor_above_its_lowest_ground() {
        // Lowest ground under the footprint is x = 10 (2 m); the flat base sits there, so the
        // building is at least one floor (FLOOR_HEIGHT_M, 3 m) above it, even with one level.
        let lowest = slope(10.0, 0.0);
        for levels in [0u8, 1, 2] {
            let m = one(building(rect(10.0, 10.0, 20.0, 20.0), levels, 0, 0));
            let top = top_y_of(&m);
            assert!(
                top - lowest >= FLOOR_HEIGHT_M as f32 - 1e-4,
                "levels {levels}: {} m above the lowest ground",
                top - lowest
            );
        }
    }

    #[test]
    fn wall_clearance_is_the_height_above_the_terrain_under_each_vertex() {
        let m = one(building(rect(10.0, 10.0, 20.0, 20.0), 2, 0, 0));
        let mut below_one_m = 0;
        for ((v, uv2), n) in m.vertices.iter().zip(&m.uv2).zip(&m.normals) {
            if n[1] != 0.0 {
                continue; // roof
            }
            let ground = slope(v[0] as f64, 0.0);
            let clearance = uv2[1];
            assert!(clearance >= 0.0, "wall vertex under the terrain: {v:?}");
            assert!(
                (clearance - (v[1] - ground)).abs() < 1e-4,
                "clearance {clearance} at {v:?}, terrain {ground}"
            );
            if clearance < 1.0 {
                below_one_m += 1;
            }
        }
        // The bottom row (on the terrain) and the high side's first meter get no window.
        assert!(below_one_m >= 4, "no wall vertex under 1 m of ground");
    }

    fn dist2(a: [f64; 2], b: [f64; 2]) -> f64 {
        (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
    }

    /// Unique wall top vertices in ring order, as local (x, y) (z is -y in Godot space).
    fn wall_ring(m: &MeshData) -> Vec<[f64; 2]> {
        let top = top_y_of(m);
        let mut out: Vec<[f64; 2]> = Vec::new();
        for (v, n) in m.vertices.iter().zip(&m.normals) {
            if n[1] != 0.0 || (v[1] - top).abs() > 1e-4 {
                continue;
            }
            let p = [v[0] as f64, -(v[2] as f64)];
            if !out.iter().any(|q| dist2(*q, p) < 1e-8) {
                out.push(p);
            }
        }
        out
    }

    /// Strict crossing of two segments (shared endpoints do not count).
    fn proper_cross(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
        let o1 = cross2(a, b, c);
        let o2 = cross2(a, b, d);
        let o3 = cross2(c, d, a);
        let o4 = cross2(c, d, b);
        ((o1 > 1e-12 && o2 < -1e-12) || (o1 < -1e-12 && o2 > 1e-12))
            && ((o3 > 1e-12 && o4 < -1e-12) || (o3 < -1e-12 && o4 > 1e-12))
    }

    fn is_simple(points: &[[f64; 2]]) -> bool {
        let n = points.len();
        for i in 0..n {
            let (a, b) = (points[i], points[(i + 1) % n]);
            for j in i + 1..n {
                if j == i || (j + 1) % n == i || j == (i + 1) % n {
                    continue;
                }
                if proper_cross(a, b, points[j], points[(j + 1) % n]) {
                    return false;
                }
            }
        }
        true
    }

    #[test]
    fn every_corner_of_a_square_gets_a_chamfer_inside_the_original_edges() {
        let square = rect(10.0, 10.0, 20.0, 20.0);
        let m = one(building(square.clone(), 1, 0, 0));
        let ring = wall_ring(&m);
        assert_eq!(ring.len(), 8, "four walls plus one chamfer per corner");
        for p in &ring {
            assert!(
                p[0] >= 10.0 - 1e-6
                    && p[0] <= 20.0 + 1e-6
                    && p[1] >= 10.0 - 1e-6
                    && p[1] <= 20.0 + 1e-6,
                "chamfer point {p:?} outside the original footprint"
            );
        }
        let d = 0.22f64;
        for corner in &square {
            assert!(
                ring.iter().all(|p| dist2(*p, *corner) > 1e-8),
                "the sharp corner {corner:?} is still a wall vertex"
            );
            let near = ring
                .iter()
                .filter(|p| dist2(**p, *corner) <= d * d + 1e-6)
                .count();
            assert_eq!(near, 2, "corner {corner:?} did not get two cut points");
        }
    }

    #[test]
    fn small_thin_and_l_shaped_footprints_stay_simple() {
        let footprints = [
            ("small", rect(0.0, 0.0, 0.3, 0.3)),
            ("thin", rect(0.0, 0.0, 0.2, 5.0)),
            ("L", l_shape()),
        ];
        for (name, pts) in footprints {
            let original = signed_area(&pts).abs();
            for segments in [1u8, 2, 3] {
                let ring = rounded_ring(&pts, 0.22, segments);
                assert!(
                    ring.points.len() >= 3,
                    "{name}/{segments}: fewer than 3 vertices"
                );
                assert!(
                    is_simple(&ring.points),
                    "{name}/{segments}: self-intersecting ring"
                );
                let area = signed_area(&ring.points);
                assert!(area > 0.0, "{name}/{segments}: zero-area or reversed ring");
                // Each corner loses at most half the square of the bevel (a right-angle chamfer).
                let max_loss = pts.len() as f64 * 0.22 * 0.22 / 2.0;
                assert!(
                    original - area <= max_loss + 1e-9,
                    "{name}/{segments}: lost {:.4} m2, expected at most {max_loss:.4}",
                    original - area
                );
                assert!(
                    area > original * 0.5,
                    "{name}/{segments}: lost half the footprint"
                );
            }
        }
    }

    #[test]
    fn zero_bevel_is_the_sharp_footprint() {
        let square = rect(10.0, 10.0, 20.0, 20.0);
        let mesh = one_with(building(square.clone(), 1, 0, 0), sharp_look()).0;
        let ring = wall_ring(&mesh);
        assert_eq!(ring.len(), 4, "no chamfer without a bevel");
        for corner in &square {
            assert!(ring.iter().any(|p| dist2(*p, *corner) < 1e-8));
        }
        // Four wall quads and a two-triangle roof: exactly the shape before item 119.
        assert_eq!(mesh.indices.len() / 3, 10);
    }

    #[test]
    fn tile_triangles_grow_within_the_budget_for_a_four_corner_building() {
        let b = building(rect(10.0, 10.0, 20.0, 20.0), 3, 1, 0);
        let sharp = one_with(b.clone(), sharp_look()).0.indices.len() / 3;
        let rounded = one_with(b, rounded_look()).0.indices.len() / 3;
        // The +15% acceptance budget is read per tile. A visible chamfer costs at least two
        // triangles per corner, so the building alone cannot stay under 15% (this square goes
        // 10 -> 22); on the tile mesh it is a small share. On a 250 m tile drawn with the 10 m
        // grid (25 x 25 cells, two triangles each):
        let terrain = 2 * 25 * 25;
        let before = terrain + sharp;
        let after = terrain + rounded;
        println!(
            "building tri {sharp} -> {rounded}; tile tri {before} -> {after} (+{:.2}%)",
            (after - before) as f64 / before as f64 * 100.0
        );
        assert!(
            (after - before) as f64 / before as f64 <= 0.15,
            "tile triangles grew {} -> {}",
            before,
            after
        );
        // Building-only growth is bounded by the new corner faces: two triangles per chamfer and
        // four more roof triangles for the rounded ring.
        assert_eq!(rounded - sharp, 12);
    }

    /// Real-map triangle count (measurement only):
    /// `MEASURE_MAP=casa cargo test -p driving_the_earth_ext real_tile_building_triangle_growth -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_tile_building_triangle_growth() {
        let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../cache"));
        let id = std::env::var("MEASURE_MAP").unwrap_or_else(|_| "casa".into());
        let index = crate::world::open_map(root, &id).expect("baked map");
        let tm = index.meta.tile_size_m;
        let (mut sharp_total, mut rounded_total) = (0usize, 0usize);
        let (mut buildings, mut corners) = (0usize, 0usize);
        for &(x, y) in index.coords() {
            let Ok(tile) = index.read_tile_at(x, y) else {
                continue;
            };
            if tile.buildings.is_empty() {
                continue;
            }
            let ground = |gx: f64, gy: f64| crate::geometry::sample_height(&tile, tm, gx, gy);
            let sharp = building_mesh_with(&tile, &ground, sharp_look()).0;
            let rounded = building_mesh_with(&tile, &ground, rounded_look()).0;
            sharp_total += sharp.indices.len() / 3;
            rounded_total += rounded.indices.len() / 3;
            buildings += tile.buildings.len();
            for b in &tile.buildings {
                let mut n = b.points.len();
                if n > 1 && b.points.first() == b.points.last() {
                    n -= 1;
                }
                corners += n;
            }
        }
        println!(
            "{id}: {buildings} buildings, {corners} corners; building triangles {sharp_total} -> {rounded_total} (+{:.1}%)",
            (rounded_total - sharp_total) as f64 / sharp_total.max(1) as f64 * 100.0
        );
        assert!(rounded_total > sharp_total);
    }

    #[test]
    fn collision_prism_is_the_original_footprint_hull() {
        for pts in [rect(10.0, 10.0, 20.0, 20.0), l_shape()] {
            let (_, sharp) = one_with(building(pts.clone(), 2, 0, 0), sharp_look());
            let (_, rounded) = one_with(building(pts.clone(), 2, 0, 0), rounded_look());
            assert_eq!(sharp, rounded, "the bevel changed the collision prism");
            assert_eq!(rounded.len(), 1);
            let hull = base_corners(&rounded[0], slope(10.0, 0.0));
            for corner in &hull {
                assert!(
                    pts.iter().any(|p| dist2(*p, *corner) < 1e-6),
                    "hull corner {corner:?} is not an original footprint vertex"
                );
            }
        }
    }

    #[test]
    fn flat_shading_is_faceted_and_smooth_only_on_the_rounded_corner() {
        let square = rect(10.0, 10.0, 20.0, 20.0);
        let flat_look = WorldLook {
            bevel_segments: 3,
            ..rounded_look()
        };
        let smooth_look = WorldLook {
            flat_shading: false,
            ..flat_look
        };
        let flat = one_with(building(square.clone(), 1, 0, 0), flat_look).0;
        let smooth = one_with(building(square, 1, 0, 0), smooth_look).0;
        // Groups of four wall vertices, one group per ring segment. A group is "flat" when all of
        // its vertices share one normal.
        let constant_groups = |m: &MeshData| {
            let walls: Vec<usize> = (0..m.normals.len())
                .filter(|&i| m.normals[i][1] == 0.0)
                .collect();
            assert_eq!(walls.len() % 4, 0);
            (0..walls.len() / 4)
                .filter(|&g| {
                    let n = m.normals[walls[4 * g]];
                    walls[4 * g..4 * g + 4].iter().all(|&i| m.normals[i] == n)
                })
                .count()
        };
        let groups = flat.normals.iter().filter(|n| n[1] == 0.0).count() / 4;
        assert_eq!(
            groups, 16,
            "four corners with three segments plus four walls"
        );
        assert_eq!(
            constant_groups(&flat),
            groups,
            "flat shading is not faceted"
        );
        // Only the four long walls keep one normal; the 12 chamfer faces smooth into each other.
        assert_eq!(
            constant_groups(&smooth),
            4,
            "smooth shading bled into the flat walls"
        );
    }
}
