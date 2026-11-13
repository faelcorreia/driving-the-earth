//! Geometry unit tests: terrain grid, road ribbons and junctions on synthetic tiles.
//! Helpers `grid`, `tile`, `hill` and `ribbon_height` are shared with `world::tests`.
use super::junction::BORDER_OVERLAP_M;
use super::*;
use tileformat::{road_width_m, HeightGrid, Polyline};

pub(crate) fn grid(cols: u32, f: impl Fn(f64, f64) -> f32) -> HeightGrid {
    let last = (cols - 1) as f64;
    let mut values = Vec::new();
    for i in 0..cols {
        for j in 0..cols {
            values.push(f(j as f64 / last, i as f64 / last));
        }
    }
    HeightGrid {
        cols,
        rows: cols,
        values,
    }
}

pub(crate) fn tile(x: i32, y: i32, height: HeightGrid, roads: Vec<Polyline>) -> Tile {
    Tile {
        signs: Vec::new(),
        x,
        y,
        height,
        roads,
        buildings: Vec::new(),
        ground: Vec::new(),
        fine_height: None,
    }
}

#[test]
fn sample_height_is_linear_on_a_ramp() {
    // 3x3 samples over 20 m (cell 10 m): height = x_m + 100 (ramp along x).
    let t = tile(0, 0, grid(3, |u, _| (u * 20.0) as f32 + 100.0), Vec::new());
    assert!((sample_height(&t, 20.0, 0.0, 0.0) - 100.0).abs() < 1e-4);
    assert!((sample_height(&t, 20.0, 5.0, 7.0) - 105.0).abs() < 1e-4);
    assert!((sample_height(&t, 20.0, 20.0, 3.0) - 120.0).abs() < 1e-4);
    // Outside the tile: the edge plane is extended (the neighbor has the same edge).
    assert!((sample_height(&t, 20.0, 30.0, -9.0) - 130.0).abs() < 1e-4);
}

#[test]
fn sample_height_follows_the_mesh_diagonal() {
    // One cell (2x2 samples, 10 m): only the north-east corner is high.
    // The mesh splits the cell from north-east to south-west, so the south-west
    // half is flat and the point (5, 5) is at 0 m (bilinear would give 2.5 m).
    let t = tile(
        0,
        0,
        HeightGrid {
            cols: 2,
            rows: 2,
            values: vec![0.0, 0.0, 0.0, 10.0],
        },
        Vec::new(),
    );
    assert!((sample_height(&t, 10.0, 5.0, 5.0)).abs() < 1e-5);
    // In the north-east half: d + ... = 4 m at (7, 7).
    assert!((sample_height(&t, 10.0, 7.0, 7.0) - 4.0).abs() < 1e-4);
    // Mesh vertices give their own heights.
    assert!((sample_height(&t, 10.0, 10.0, 10.0) - 10.0).abs() < 1e-5);
}

/// Smooth hill of 8 m, 20 m wide, centered in a 250 m tile.
pub(crate) fn hill(x: f64, y: f64) -> f32 {
    (8.0 * (-((x - 125.0).powi(2) + (y - 125.0).powi(2)) / (2.0 * 20.0 * 20.0)).exp()) as f32
}

/// Height of the drawn road mesh at local `(x, y)`, by barycentric interpolation
/// over the triangle that contains the point (`None` outside all triangles).
pub(crate) fn ribbon_height(mesh: &MeshData, x: f64, y: f64) -> Option<f64> {
    for tri in mesh.indices.chunks(3) {
        let p: Vec<[f64; 3]> = tri
            .iter()
            .map(|&i| {
                let v = mesh.vertices[i as usize];
                // Godot (x, y, z) -> local (x, y) with z = -north.
                [v[0] as f64, v[1] as f64, -(v[2] as f64)]
            })
            .collect();
        let det =
            (p[1][0] - p[0][0]) * (p[2][2] - p[0][2]) - (p[2][0] - p[0][0]) * (p[1][2] - p[0][2]);
        if det.abs() < 1e-12 {
            continue;
        }
        let l1 = ((x - p[0][0]) * (p[2][2] - p[0][2]) - (p[2][0] - p[0][0]) * (y - p[0][2])) / det;
        let l2 = ((p[1][0] - p[0][0]) * (y - p[0][2]) - (x - p[0][0]) * (p[1][2] - p[0][2])) / det;
        let l0 = 1.0 - l1 - l2;
        let eps = -1e-9;
        if l0 >= eps && l1 >= eps && l2 >= eps {
            return Some(l0 * p[0][1] + l1 * p[1][1] + l2 * p[2][1]);
        }
    }
    None
}

#[test]
fn road_never_goes_under_the_terrain() {
    // A bendy road over a hill: the strip is sampled between nodes, so it must stay
    // above the drawn terrain everywhere, not only at the nodes.
    let road = Polyline {
        oneway: None,
        lanes: None,
        maxspeed: None,
        roundabout: false,
        name: String::new(),
        z: Vec::new(),
        zl: Vec::new(),
        zr: Vec::new(),
        highway: "residential".into(),
        points: vec![[60.0, 100.0], [120.0, 128.0], [190.0, 150.0], [200.0, 60.0]],
        lit: None,
    };
    let t = tile(
        0,
        0,
        grid(26, |u, v| hill(u * 250.0, v * 250.0)),
        vec![road.clone()],
    );
    let g = tile_geometry(&t, 250.0);
    let mut checked = 0;
    for k in 0..road.points.len() - 1 {
        let (p, q) = (road.points[k], road.points[k + 1]);
        for s in 0..=40 {
            for side in [-3.0, -1.5, 0.0, 1.5, 3.0] {
                let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
                let len = (dx * dx + dy * dy).sqrt();
                let t_ = s as f64 / 40.0;
                // Stay off the nodes of the polyline: the bend is covered by the miter.
                let x = p[0] + dx * t_ + -dy / len * side;
                let y = p[1] + dy * t_ + dx / len * side;
                let Some(strip) = ribbon_height(&g.roads, x, y) else {
                    continue;
                };
                let terrain = sample_height(&t, 250.0, x, y) as f64;
                // ROAD_LIFT_M covers the curvature between stations, but this synthetic hill is much
                // steeper than real terrain, so allow a few centimeters.
                assert!(
                    strip >= terrain - 0.05,
                    "road under terrain at ({x:.2}, {y:.2}): {strip} < {terrain}"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 400, "only {checked} points checked");
}

#[test]
fn road_with_profile_follows_its_heights() {
    // Flat terrain at 0 m, road heights ramp 0 -> 10 m: the ribbon follows the profile.
    let road = Polyline {
        oneway: None,
        lanes: None,
        maxspeed: None,
        roundabout: false,
        name: String::new(),
        highway: "residential".into(),
        points: vec![[60.0, 125.0], [190.0, 125.0]],
        z: vec![0.0, 10.0],
        zl: Vec::new(),
        zr: Vec::new(),
        lit: None,
    };
    let t = tile(0, 0, grid(26, |_, _| 0.0), vec![road]);
    let g = tile_geometry(&t, 250.0);
    for x in [70.0, 125.0, 180.0] {
        let strip = ribbon_height(&g.roads, x, 125.0).expect("road covers the point");
        let want = (x - 60.0) / 130.0 * 10.0 + 0.1;
        // The ribbon rises slightly above the profile on steep stretches (`ROAD_STEP_LIFT`).
        assert!((strip - want).abs() < 0.03, "x={x}: {strip} != {want}");
    }
}

#[test]
fn terrain_under_a_profiled_road_follows_its_profile() {
    // A crest of 2 m across the road at x = 125; the road profile is flat at 0 m.
    let crest = |u: f64, _v: f64| {
        let x = u * 250.0;
        (2.0 * (-((x - 125.0) / 6.0).powi(2)).exp()) as f32
    };
    let road = Polyline {
        oneway: None,
        lanes: None,
        maxspeed: None,
        roundabout: false,
        name: String::new(),
        highway: "residential".into(),
        points: vec![[0.0, 125.0], [250.0, 125.0]],
        z: vec![0.0, 0.0],
        zl: Vec::new(),
        zr: Vec::new(),
        lit: None,
    };
    let raw = tile(0, 0, grid(26, crest), Vec::new());
    let with = tile(0, 0, grid(26, crest), vec![road.clone()]);
    let plain = tile_geometry(&raw, 250.0);
    let g = tile_geometry(&with, 250.0);
    assert_eq!(
        g.terrain.indices, plain.terrain.indices,
        "triangles must not change"
    );
    let mut on_axis = 0;
    for (v, w) in g.terrain.vertices.iter().zip(&plain.terrain.vertices) {
        let (x, y) = (v[0] as f64, -(v[2] as f64));
        let dist = (y - 125.0).abs();
        if dist <= 3.0 {
            assert!(
                v[1].abs() < 1e-4,
                "vertex ({x:.1}, {y:.1}) is {} off the profile",
                v[1]
            );
            on_axis += 1;
        } else if dist > 7.5 + TERRAIN_FINE_CELL_M {
            // flat zone of a 6 m road: flat_zone_m(3.0)
            assert_eq!(
                v[1], w[1],
                "vertex outside the band changed at ({x:.1}, {y:.1})"
            );
        }
    }
    assert!(on_axis > 0);
}

#[test]
fn road_ends_meet_across_tiles() {
    // Two tiles share the edge x = 250 m: a road crossing it gives a piece on each side.
    // Their end vertices are the same point, with the same height from both tiles.
    // Grid values are global heights: tile (0, 0) spans x in [0, 250], tile (1, 0) in [250, 500].
    let left = tile(0, 0, grid(26, |u, v| hill(u * 250.0, v * 250.0)), vec![]);
    let right = tile(
        1,
        0,
        grid(26, |u, v| hill(250.0 + u * 250.0, v * 250.0)),
        vec![],
    );
    for y in [0.0, 60.0, 125.0, 200.0] {
        let a = sample_height(&left, 250.0, 250.0, y);
        let b = sample_height(&right, 250.0, 250.0, y);
        assert!(
            (a - b).abs() < 1e-4,
            "edge height differs at y={y}: {a} vs {b}"
        );
    }
    // The strip of each tile ends on the shared border line.
    let piece = |x0: f64, x1: f64| Polyline {
        oneway: None,
        lanes: None,
        maxspeed: None,
        roundabout: false,
        name: String::new(),
        z: Vec::new(),
        zl: Vec::new(),
        zr: Vec::new(),
        highway: "residential".into(),
        points: vec![[x0, 125.0], [x1, 125.0]],
        lit: None,
    };
    let gl = tile_geometry(
        &tile(
            0,
            0,
            grid(26, |u, v| hill(u * 250.0, v * 250.0)),
            vec![piece(100.0, 250.0)],
        ),
        250.0,
    );
    let gr = tile_geometry(
        &tile(
            1,
            0,
            grid(26, |u, v| hill(250.0 + u * 250.0, v * 250.0)),
            vec![piece(250.0, 400.0)],
        ),
        250.0,
    );
    let edge_x = |m: &MeshData| {
        m.vertices
            .iter()
            .filter(|v| (v[0] - 250.0).abs() < 1e-4)
            .map(|v| (v[1], v[2]))
            .collect::<Vec<_>>()
    };
    assert!(!edge_x(&gl.roads).is_empty() && !edge_x(&gr.roads).is_empty());
    for (y1, z1) in edge_x(&gl.roads) {
        assert!(edge_x(&gr.roads)
            .iter()
            .any(|&(y2, z2)| (y1 - y2).abs() < 1e-4 && (z1 - z2).abs() < 1e-4));
    }
}

#[test]
fn road_height_is_continuous_across_the_tile_border() {
    // A 80-point profile (2.5 m stations, DEM-like noise) along y = 125 crosses the border x = 250
    // between tile (0, 0), which owns the road, and tile (1, 0), which sees it as a neighbor.
    let n = 80;
    let points: Vec<[f64; 2]> = (0..n).map(|k| [150.0 + 2.5 * k as f64, 125.0]).collect();
    let z: Vec<f32> = points
        .iter()
        .enumerate()
        .map(|(k, p)| (0.04 * (p[0] - 150.0) + if k % 2 == 0 { 0.08 } else { -0.08 }) as f32)
        .collect();
    let (smooth, _, _) =
        super::profile_smooth::smooth_profile(&points, &z, &[], &[], "residential");
    let road = Polyline {
        oneway: None,
        lanes: None,
        maxspeed: None,
        roundabout: false,
        name: String::new(),
        highway: "residential".into(),
        points,
        z,
        zl: Vec::new(),
        zr: Vec::new(),
        lit: None,
    };
    let half = road_width_m("residential") / 2.0;
    let t0 = tile(
        0,
        0,
        grid(26, |u, v| hill(u * 250.0, v * 250.0)),
        vec![road],
    );
    let t1 = tile(
        1,
        0,
        grid(26, |u, v| hill(250.0 + u * 250.0, v * 250.0)),
        Vec::new(),
    );
    let g0 = tile_geometry_with(&t0, 250.0, &|x, y| sample_height(&t0, 250.0, x, y), &[&t1]);
    let g1 = tile_geometry_with(&t1, 250.0, &|x, y| sample_height(&t1, 250.0, x, y), &[&t0]);

    // Ribbon of the owning tile follows the smoothed profile on both sides of the border.
    for x in (160..=340).step_by(5) {
        let x = x as f64;
        let Some(strip) = ribbon_height(&g0.roads, x, 125.0) else {
            continue;
        };
        let k = ((x - 150.0) / 2.5).round() as usize;
        let want = smooth[k] as f64 + ROAD_LIFT_M as f64;
        assert!(
            (strip - want).abs() <= 0.02,
            "ribbon {strip:.3} off the profile {want:.3} at x={x}"
        );
    }
    // The border itself: the owning tile's ribbon and the neighbor's terrain agree there.
    let on_border = ribbon_height(&g0.roads, 250.0, 125.0).expect("ribbon on the border");
    let k = ((250.0 - 150.0) / 2.5) as usize;
    assert!((on_border - (smooth[k] as f64 + ROAD_LIFT_M as f64)).abs() <= 0.02);

    // Terrain of the neighbor on the axis, past the border, never rises above the ribbon of (0, 0).
    let mut checked = 0;
    for v in &g1.terrain.vertices {
        let (x, y, h) = (v[0] as f64, -(v[2] as f64), v[1] as f64);
        if (y - 125.0).abs() > half || !(250.0..=340.0).contains(&x) {
            continue;
        }
        if let Some(strip) = ribbon_height(&g0.roads, x, y) {
            checked += 1;
            assert!(
                h - strip <= 0.02,
                "terrain {h:.3} above the ribbon {strip:.3} at ({x:.1}, {y:.1})"
            );
        }
    }
    assert!(
        checked > 0,
        "no neighbor terrain vertex under the ribbon past the border"
    );
}

#[test]
fn sample_height_uses_tile_offset() {
    // Tile (1, -1) starts at x = 20 m, y = -20 m.
    let t = tile(1, -1, grid(3, |_, _| 7.5), Vec::new());
    assert!((sample_height(&t, 20.0, 30.0, -10.0) - 7.5).abs() < 1e-6);
}

#[test]
fn terrain_is_a_grid_with_front_faces_up() {
    let t = tile(0, 0, grid(5, |u, v| (u * 3.0 + v * 2.0) as f32), Vec::new());
    let g = tile_geometry(&t, 20.0);
    assert_eq!(g.terrain.vertices.len(), 81);
    assert_eq!(g.terrain.normals.len(), 81);
    assert_eq!(g.terrain.indices.len(), 2 * 8 * 8 * 3);
    assert!(g.terrain.indices.iter().all(|&i| (i as usize) < 81));
    for tri in g.terrain.indices.chunks(3) {
        let p: Vec<[f32; 3]> = tri
            .iter()
            .map(|&i| g.terrain.vertices[i as usize])
            .collect();
        let cross_y =
            (p[1][2] - p[0][2]) * (p[2][0] - p[0][0]) - (p[1][0] - p[0][0]) * (p[2][2] - p[0][2]);
        assert!(cross_y < 0.0, "triangle {tri:?} is back-facing for Godot");
    }
}

#[test]
fn terrain_normals_follow_the_slope() {
    // Height rises 0.5 m per meter eastward: interior normals tilt west (-x).
    let t = tile(0, 0, grid(5, |u, _| (u * 20.0 * 0.5) as f32), Vec::new());
    let g = tile_geometry(&t, 20.0);
    let n = g.terrain.normals[4 * 9 + 4];
    let len = (0.5f32 * 0.5 + 1.0).sqrt();
    assert!((n[0] + 0.5 / len).abs() < 1e-4, "{n:?}");
    assert!((n[1] - 1.0 / len).abs() < 1e-4, "{n:?}");
    assert!(n[2].abs() < 1e-4, "{n:?}");
}

#[test]
fn terrain_world_axes_match_godot() {
    // Local (x = 10, y = 20) is east 10 m and north 20 m: Godot (10, h, -20).
    let t = tile(0, 1, grid(3, |_, _| 4.0), Vec::new());
    let g = tile_geometry(&t, 20.0);
    // Vertex row 0 is at y = 20 (tile y = 1), column 1 at x = 10.
    assert_eq!(g.terrain.vertices[4], [10.0, 4.0, -20.0]);
}

#[test]
fn road_becomes_front_up_ribbon_on_terrain() {
    let road = Polyline {
        oneway: None,
        lanes: None,
        maxspeed: None,
        roundabout: false,
        name: String::new(),
        z: Vec::new(),
        zl: Vec::new(),
        zr: Vec::new(),
        highway: "residential".into(),
        points: vec![[0.0, 10.0], [20.0, 10.0]],
        lit: None,
    };
    let t = tile(0, 0, grid(3, |_, _| 50.0), vec![road]);
    let g = tile_geometry(&t, 20.0);
    // Both ends lie on tile borders, so the ribbon runs BORDER_OVERLAP_M past each.
    let xs: Vec<f32> = g.roads.vertices.iter().map(|v| v[0]).collect();
    let (lo, hi) = (BORDER_OVERLAP_M as f32, 20.0 + BORDER_OVERLAP_M as f32);
    assert!(xs.iter().any(|x| (x + lo).abs() < 1e-4));
    assert!(xs.iter().any(|x| (x - hi).abs() < 1e-4));
    assert_eq!(
        g.roads.indices.len() / 3,
        (g.roads.vertices.len() / 3 - 1) * 4
    );
    // Vertices come in (left edge, center, right edge) triples: the center keeps the lift, the edges fall to the terrain.
    for (i, v) in g.roads.vertices.iter().enumerate() {
        let want = if i % 3 == 1 { ROAD_LIFT_M } else { super::road::ROAD_EDGE_LIFT_M };
        assert!((v[1] - (50.0 + want)).abs() < 1e-4);
    }
    // Ribbon is 6 m wide: corners at z = -(10 +- 3).
    let zs: Vec<f32> = g.roads.vertices.iter().map(|v| v[2]).collect();
    assert!(zs.iter().any(|z| (z + 13.0).abs() < 1e-4));
    assert!(zs.iter().any(|z| (z + 7.0).abs() < 1e-4));
    for tri in g.roads.indices.chunks(3) {
        let p: Vec<[f32; 3]> = tri.iter().map(|&i| g.roads.vertices[i as usize]).collect();
        let cross_y =
            (p[1][2] - p[0][2]) * (p[2][0] - p[0][0]) - (p[1][0] - p[0][0]) * (p[2][2] - p[0][2]);
        assert!(cross_y < 0.0);
    }
}

/// A 12 m primary road ends on the centerline of a 6 m residential road, at 45 degrees.
/// Its cut is perpendicular to it, so the corner on the near side pokes out of the
/// crossed road and leaves a wedge uncovered between the cut and the crossed road's
/// edge. Points of A's band beyond its end, outside the crossed road on A's side, must
/// be covered.
#[test]
fn road_end_on_crossed_road_leaves_no_wedge() {
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let end = [125.0, 125.0];
    let a = Polyline {
        oneway: None,
        lanes: None,
        maxspeed: None,
        roundabout: false,
        name: String::new(),
        z: Vec::new(),
        zl: Vec::new(),
        zr: Vec::new(),
        highway: "primary".into(),
        points: vec![[end[0] - 80.0 * s, end[1] - 80.0 * s], end],
        lit: None,
    };
    let b = Polyline {
        oneway: None,
        lanes: None,
        maxspeed: None,
        roundabout: false,
        name: String::new(),
        z: Vec::new(),
        zl: Vec::new(),
        zr: Vec::new(),
        highway: "residential".into(),
        points: vec![[0.0, 125.0], [250.0, 125.0]],
        lit: None,
    };
    let t = tile(0, 0, grid(26, |_, _| 50.0), vec![a, b]);
    let g = tile_geometry(&t, 250.0);
    let (half_a, half_b) = (6.0, 3.0);
    let mut missing = Vec::new();
    for ia in 0..=24 {
        for ib in 0..=24 {
            // Along the road (a from the end, up to half_b past it) and across it.
            let along = half_b * ia as f64 / 24.0;
            let across = (half_a - 0.2) * (2.0 * ib as f64 / 24.0 - 1.0);
            let x = end[0] + along * s - across * s;
            let y = end[1] + along * s + across * s;
            // Near side of the crossed road only (its far side may be overshot).
            if y > end[1] - half_b {
                continue;
            }
            if ribbon_height(&g.roads, x, y).is_none() {
                missing.push((x, y));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "{} uncovered points, e.g. {:?}",
        missing.len(),
        missing.first()
    );
}

/// FNV-1a over the raw bytes of the terrain and road meshes (vertices, normals, indices).
fn mesh_hash(mesh: &MeshData) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for b in bytes {
            h = (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    };
    for v in mesh
        .vertices
        .iter()
        .flatten()
        .chain(mesh.normals.iter().flatten())
    {
        feed(&v.to_le_bytes());
    }
    for i in &mesh.indices {
        feed(&i.to_le_bytes());
    }
    h
}

/// Regression: the terrain and road positions, normals and indices of the reference tile
/// (`cache/casa` tile 0,0) are byte-identical to the ones before the color channel. Only the
/// terrain colors are new (from the ground cover); the roads have none. The road hash also pins
/// the junction weld: roads meeting at a node share one height there (item 34).
#[test]
fn reference_tile_geometry_is_unchanged() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache/casa/tiles/0_0.json");
    let Ok(tile) = tileformat::read_tile(&path) else {
        eprintln!("skipped: {} not found", path.display());
        return;
    };
    let g = tile_geometry(&tile, 250.0);
    assert!(g.roads.colors.is_empty());
    assert_eq!(g.terrain.colors.len(), g.terrain.vertices.len());
    println!(
        "terrain {:016x} roads {:016x}",
        mesh_hash(&g.terrain),
        mesh_hash(&g.roads)
    );
    assert_eq!(mesh_hash(&g.terrain), REFERENCE_TERRAIN_HASH);
    assert_eq!(mesh_hash(&g.roads), REFERENCE_ROADS_HASH);
}

const REFERENCE_TERRAIN_HASH: u64 = 0x3277fb90883f3355;
const REFERENCE_ROADS_HASH: u64 = 0xf3ec4bbc192c2b47;

/// A tile with no roads has the 10 m grid, bilinear, at every fine vertex (2,5 m).
#[test]
fn tile_without_roads_is_the_grid_on_the_fine_vertices() {
    let t = tile(0, 0, grid(9, |u, v| hill(u * 250.0, v * 250.0)), Vec::new());
    let g = tile_geometry(&t, 250.0);
    assert!(g.roads.is_empty());
    let n = 101;
    assert_eq!(g.terrain.vertices.len(), n * n);
    for (k, v) in g.terrain.vertices.iter().enumerate() {
        let (x, y) = (
            (k % n) as f64 * TERRAIN_FINE_CELL_M,
            (k / n) as f64 * TERRAIN_FINE_CELL_M,
        );
        assert_eq!(v[1], sample_height(&t, 250.0, x, y), "vertex {k}");
    }
}

/// Largest `terrain vertex - ribbon` over the terrain vertices inside the road's corridor
/// (`|offset| <= half`) of a straight road along x at y = 125. `None` when no vertex has a ribbon.
#[cfg(test)]
fn worst_vertex_above_ribbon(g: &TileGeometry, y0: f64, half: f64) -> Option<(f64, f64, f64)> {
    let mut worst: Option<(f64, f64, f64)> = None;
    for v in &g.terrain.vertices {
        let (x, y, h) = (v[0] as f64, -(v[2] as f64), v[1] as f64);
        if (y - y0).abs() > half {
            continue;
        }
        let Some(strip) = ribbon_height(&g.roads, x, y) else {
            continue;
        };
        let excess = h - strip;
        if worst.map_or(true, |w| excess > w.0) {
            worst = Some((excess, x, y));
            eprintln!("  vertex h={h:.3} ribbon={strip:.3} at ({x:.2}, {y:.2})");
        }
    }
    worst
}

#[test]
fn no_terrain_vertex_above_the_ribbon_in_the_corridor() {
    // Hill across the road, profile flat (below the hill crest), and profile ramping over the hill.
    let half = road_width_m("residential") / 2.0;
    for (name, z) in [
        ("flat", vec![0.0f32, 0.0]),
        ("ramp", vec![-2.0f32, 6.0]),
        ("fill", vec![9.0f32, 9.0]),
    ] {
        let road = Polyline {
            oneway: None,
            lanes: None,
            maxspeed: None,
            roundabout: false,
            name: String::new(),
            highway: "residential".into(),
            // Runs past both tile edges: a road that ends on an edge dips toward the grid in its
            // end cap, which is not a corridor case.
            points: vec![[-20.0, 125.0], [270.0, 125.0]],
            z,
            zl: Vec::new(),
            zr: Vec::new(),
            lit: None,
        };
        let t = tile(
            0,
            0,
            grid(26, |u, v| hill(u * 250.0, v * 250.0)),
            vec![road],
        );
        let g = tile_geometry(&t, 250.0);
        let (excess, x, y) = worst_vertex_above_ribbon(&g, 125.0, half).expect("corridor vertices");
        eprintln!("{name}: worst vertex above ribbon {excess:.4} m at ({x:.1}, {y:.1})");
        assert!(
            excess <= 0.0,
            "{name}: terrain vertex {excess:.4} m above the ribbon at ({x:.1}, {y:.1})"
        );
    }
}

/// Release-only measurement: `cargo test --release -p driving_the_earth_ext tile_geometry_timing -- --ignored --nocapture`.
#[test]
#[ignore]
fn tile_geometry_timing() {
    let roads: Vec<Polyline> = (0..4)
        .map(|k| Polyline {
            oneway: None,
            lanes: None,
            maxspeed: None,
            roundabout: false,
            name: String::new(),
            highway: "residential".into(),
            points: vec![
                [-20.0, 40.0 + 55.0 * k as f64],
                [270.0, 60.0 + 55.0 * k as f64],
            ],
            z: vec![-2.0, 6.0],
            zl: Vec::new(),
            zr: Vec::new(),
            lit: None,
        })
        .collect();
    let t = tile(0, 0, grid(26, |u, v| hill(u * 250.0, v * 250.0)), roads);
    let mut times = Vec::new();
    let mut tris = 0;
    for _ in 0..200 {
        let start = std::time::Instant::now();
        let g = tile_geometry(&t, 250.0);
        times.push(start.elapsed().as_secs_f64() * 1e3);
        tris = g.terrain.indices.len() / 3 + g.roads.indices.len() / 3;
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p90 = times[times.len() * 9 / 10];
    eprintln!("tile_geometry p90 {p90:.3} ms, tri {tris}");
}

/// Probe (S1c): a many-point profile (2.5 m stations, DEM-like noise) on flat ground.
/// The ribbon on the axis must follow the smoothed profile, not the grid (0 m).
#[test]
fn probe_many_point_profile_on_axis() {
    let n = 117;
    let points: Vec<[f64; 2]> = (0..n).map(|k| [-20.0 + 2.5 * k as f64, 125.0]).collect();
    let z: Vec<f32> = points
        .iter()
        .enumerate()
        .map(|(k, p)| (0.04 * (p[0] - 125.0) + if k % 2 == 0 { 0.08 } else { -0.08 }) as f32)
        .collect();
    let (smooth, _, _) =
        super::profile_smooth::smooth_profile(&points, &z, &[], &[], "residential");
    let road = Polyline {
        oneway: None,
        lanes: None,
        maxspeed: None,
        roundabout: false,
        name: String::new(),
        highway: "residential".into(),
        points: points.clone(),
        z: z.clone(),
        zl: Vec::new(),
        zr: Vec::new(),
        lit: None,
    };
    let t = tile(0, 0, grid(26, |_, _| 0.0), vec![road]);
    let g = tile_geometry(&t, 250.0);
    let mut worst = (0.0f64, 0.0f64, 0.0f64);
    for x in (15..=235).step_by(5) {
        let x = x as f64;
        let strip = ribbon_height(&g.roads, x, 125.0).expect("on the ribbon");
        let k = ((x + 20.0) / 2.5).round() as usize;
        let want = smooth[k] as f64 + ROAD_LIFT_M as f64;
        let err = (strip - want).abs();
        eprintln!("x={x} ribbon={strip:.3} want={want:.3}");
        if err > worst.0 {
            worst = (err, x, strip);
        }
    }
    eprintln!(
        "probe worst error {:.4} m at x={} (ribbon {:.4})",
        worst.0, worst.1, worst.2
    );
    assert!(
        worst.0 <= 0.02,
        "axis off the smoothed profile by {:.4} m at x={} (ribbon {:.4})",
        worst.0,
        worst.1,
        worst.2
    );
}

/// Diagnostic on the real baked map `casa-10km`, tile (-1, -2) (run with `--ignored --nocapture`):
/// the terrain of the tile must not rise above any road ribbon of the 3 x 3 block around it.
#[test]
#[ignore]
fn real_tile_terrain_stays_under_the_ribbons() {
    let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../cache"));
    let index = crate::world::open_map(root, "casa-10km").expect("baked map 'casa-10km'");
    let m = index.meta.tile_size_m;
    let (cx, cy): (i32, i32) = std::env::var("TILE")
        .ok()
        .and_then(|v| {
            let (a, b) = v.split_once(',')?;
            Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
        })
        .unwrap_or((-1, -2));
    let block: Vec<Tile> = (-1..=1)
        .flat_map(|dy| (-1..=1).map(move |dx| (cx + dx, cy + dy)))
        .filter_map(|(x, y)| index.read_tile_at(x, y).ok())
        .collect();
    let center = block
        .iter()
        .find(|t| t.x == cx && t.y == cy)
        .expect("center tile");
    let geo: Vec<TileGeometry> = block
        .iter()
        .map(|t| {
            let around: Vec<&Tile> = block.iter().filter(|o| (o.x, o.y) != (t.x, t.y)).collect();
            tile_geometry_with(t, m, &|x, y| sample_height(t, m, x, y), &around)
        })
        .collect();
    let g = &geo[block.iter().position(|t| std::ptr::eq(t, center)).unwrap()];
    let (mut worst, mut checked) = (f64::NEG_INFINITY, 0usize);
    for v in &g.terrain.vertices {
        let (x, y, h) = (v[0] as f64, -(v[2] as f64), v[1] as f64);
        let strips: Vec<f64> = geo
            .iter()
            .filter_map(|k| ribbon_height(&k.roads, x, y))
            .collect();
        if strips.is_empty() {
            continue;
        }
        checked += 1;
        let lowest = strips.iter().cloned().fold(f64::INFINITY, f64::min);
        let excess = h - lowest;
        if excess > worst {
            worst = excess;
        }
        if excess > 0.02 {
            println!("leak x={x:.2} y={y:.2} h={h:.3} excess={excess:.3}");
        }
    }
    println!("checked={checked} worst_excess={worst:.4} m");
    assert!(
        checked > 0,
        "no terrain vertex under a ribbon in tile (-1, -2)"
    );
    assert!(
        worst <= 0.02,
        "terrain {worst:.4} m above the ribbon in tile (-1, -2)"
    );
}

/// P1: the ribbon mesh against its profile, on the axis and on both edges, within 2 cm. A road
/// with a grade (linear legs) and a cross slope of 5 cm per edge on flat ground. The edge
/// height is linear across the half width (`zl`/`zr` at the edges, `z` on the axis).
#[test]
fn ribbon_mesh_matches_profile_on_axis_and_edges_within_2cm() {
    let y0 = 125.0;
    let road = Polyline {
        oneway: None,
        lanes: None,
        maxspeed: None,
        roundabout: false,
        name: String::new(),
        highway: "residential".into(),
        points: vec![[30.0, y0], [100.0, y0], [220.0, y0]],
        z: vec![0.0, 0.7, 1.4],
        zl: vec![0.05, 0.75, 1.45],
        zr: vec![-0.05, 0.65, 1.35],
        lit: None,
    };
    let half = tileformat::road_width_m(&road.highway) / 2.0;
    let t = tile(0, 0, grid(26, |_, _| 0.0), vec![road]);
    let g = tile_geometry(&t, 250.0);
    let lift = ROAD_LIFT_M as f64;
    let edge_lift = super::road::ROAD_EDGE_LIFT_M as f64;
    let axis_z = |x: f64| {
        if x <= 100.0 {
            (x - 30.0) / 70.0 * 0.7
        } else {
            0.7 + (x - 100.0) / 120.0 * 0.7
        }
    };
    let (mut checked, mut worst) = (0, 0.0f64);
    for x in (40..=210).step_by(5) {
        let x = x as f64;
        for (s, offset) in [(0.0, 0.0), (0.8 * half, 0.05), (-0.8 * half, -0.05)] {
            let Some(h) = ribbon_height(&g.roads, x, y0 + s) else {
                continue;
            };
            // `offset` is the edge's cross slope; the lateral share of it is |s| / half. The lift falls
            // from the center's to the edge's linearly over the half width (the crown).
            let f = s.abs() / half;
            let want = axis_z(x) + offset * f + lift + (edge_lift - lift) * f;
            checked += 1;
            worst = worst.max((h - want).abs());
        }
    }
    println!("ribbon vs profile: {checked} points, worst {worst:.4} m");
    assert!(checked > 100, "only {checked} points checked");
    assert!(
        worst <= 0.02,
        "worst deviation {worst:.4} m > 0.02 m over {checked} points"
    );
}

/// Percentile `p` (0..1) by nearest rank; 0 for an empty slice. Sorts `v`.
fn ribbon_percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// Population standard deviation; 0 for an empty slice.
fn ribbon_std(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let m = v.iter().sum::<f64>() / v.len() as f64;
    (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
}

/// Ribbon metrics on the real baked map (measurement only, not a pass/fail test). Takes the
/// centerline vertices of every road ribbon (`MEASURE_MAP`, default `casa`), resampled every
/// 2.5 m, within 500 m of home (the map origin), and prints pitch, steps, lumps, the axis above
/// the sample_height grid and the 15 worst steps.
/// Run: `cargo test --release -p driving_the_earth_ext ribbon_metrics -- --ignored --nocapture`.
#[test]
#[ignore]
fn ribbon_metrics() {
    const STEP_M: f64 = 2.5;
    const HOME_RADIUS_M: f64 = 500.0;
    const LUMP_HALF_WINDOW: usize = 8; // 8 samples = 20 m each side
    let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../cache"));
    let id = std::env::var("MEASURE_MAP").unwrap_or_else(|_| "casa".into());
    let index = crate::world::open_map(root, &id).expect("open map");
    let tm = index.meta.tile_size_m;

    // Pass 1: raw centerline runs per tile, and each tile's grid (the 10 m heights `sample_height` reads).
    let mut raw_runs: Vec<((i32, i32), Vec<([f64; 2], f64)>)> = Vec::new();
    let mut grids: std::collections::HashMap<(i32, i32), Tile> = std::collections::HashMap::new();
    for &(x, y) in index.coords() {
        let g = index.tile_geometry(x, y).expect("tile_geometry");
        let v = &g.roads.vertices;
        let mut run: Vec<([f64; 2], f64)> = Vec::new();
        for s in 0..v.len() / 3 {
            // Center vertex of station s; map y is -z in Godot space.
            let c = v[3 * s + 1];
            let p = [c[0] as f64, -(c[2] as f64)];
            if let Some(last) = run.last() {
                if (p[0] - last.0[0]).hypot(p[1] - last.0[1]) > 2.6 {
                    raw_runs.push(((x, y), std::mem::take(&mut run)));
                }
            }
            run.push((p, c[1] as f64));
        }
        if !run.is_empty() {
            raw_runs.push(((x, y), run));
        }
        grids.insert((x, y), index.read_tile_at(x, y).expect("read tile"));
    }

    // Grid height under a map point: `sample_height` of the tile that contains it (the 10 m grid,
    // not the drawn terrain). `None` when no tile covers the point.
    let terrain_at = |p: [f64; 2]| -> Option<f64> {
        let key = ((p[0] / tm).floor() as i32, (p[1] / tm).floor() as i32);
        let tile = grids.get(&key)?;
        Some(sample_height(tile, tm, p[0], p[1]) as f64)
    };
    // Distance in m from a map point to the nearest tile border.
    let border_gap = |p: [f64; 2]| -> f64 {
        let fx = p[0] - (p[0] / tm).floor() * tm;
        let fy = p[1] - (p[1] / tm).floor() * tm;
        fx.min(tm - fx).min(fy).min(tm - fy)
    };

    // Pass 2: resample every run every 2.5 m, keeping the tile it came from.
    let mut samples: Vec<((i32, i32), Vec<([f64; 2], f64)>)> = Vec::new();
    for (key, run) in &raw_runs {
        if run.len() < 2 {
            continue;
        }
        let mut out = Vec::new();
        let (mut s_acc, mut next) = (0.0f64, 0.0f64);
        for k in 0..run.len() - 1 {
            let (a, b) = (run[k], run[k + 1]);
            let len = (b.0[0] - a.0[0]).hypot(b.0[1] - a.0[1]);
            while next <= s_acc + len + 1e-9 {
                let t = if len > 0.0 { (next - s_acc) / len } else { 0.0 };
                out.push((
                    [
                        a.0[0] + (b.0[0] - a.0[0]) * t,
                        a.0[1] + (b.0[1] - a.0[1]) * t,
                    ],
                    a.1 + (b.1 - a.1) * t,
                ));
                next += STEP_M;
            }
            s_acc += len;
        }
        samples.push((*key, out));
    }

    let near = |p: [f64; 2]| p[0].hypot(p[1]) <= HOME_RADIUS_M;
    let heading = |p: [f64; 2], q: [f64; 2]| (q[1] - p[1]).atan2(q[0] - p[0]);
    // Heading change in degrees over `k` samples on each side of `i`.
    let turn = |ps: &[([f64; 2], f64)], i: usize, k: usize| -> f64 {
        let d = (heading(ps[i - k].0, ps[i].0) - heading(ps[i].0, ps[i + k].0)).abs();
        d.min(2.0 * std::f64::consts::PI - d).to_degrees()
    };

    let (mut pitch_straight, mut pitch_curve): (Vec<f64>, Vec<f64>) = (vec![], vec![]);
    let (mut lump_straight, mut lump_curve): (Vec<f64>, Vec<f64>) = (vec![], vec![]);
    // Every near step: (second difference in m, position, tile key).
    let mut steps: Vec<(f64, [f64; 2], (i32, i32))> = Vec::new();
    let mut axis_above_terrain = (f64::NEG_INFINITY, [0.0f64; 2]);
    let mut axis_no_terrain = 0usize;
    for (key, ps) in &samples {
        let n = ps.len();
        if n < 12 {
            continue;
        }
        // Slope angle of segment j, in degrees.
        let slope = |j: usize| {
            let d = (ps[j + 1].0[0] - ps[j].0[0]).hypot(ps[j + 1].0[1] - ps[j].0[1]);
            ((ps[j + 1].1 - ps[j].1) / d).atan().to_degrees()
        };
        for i in 4..n - 4 {
            if !near(ps[i].0) {
                continue;
            }
            // Pitch: change of slope between consecutive segments.
            let pitch = (slope(i) - slope(i - 1)).abs();
            // Step: second difference of height across the station.
            let step = (ps[i + 1].1 - 2.0 * ps[i].1 + ps[i - 1].1).abs();
            steps.push((step, ps[i].0, *key));
            match terrain_at(ps[i].0) {
                Some(t) => {
                    let above = ps[i].1 - t;
                    if above > axis_above_terrain.0 {
                        axis_above_terrain = (above, ps[i].0);
                    }
                }
                None => axis_no_terrain += 1,
            }
            if turn(ps, i, 4) > 10.0 {
                pitch_curve.push(pitch);
            } else {
                pitch_straight.push(pitch);
            }
        }
        for i in LUMP_HALF_WINDOW + 2..n - LUMP_HALF_WINDOW - 2 {
            if !near(ps[i].0) {
                continue;
            }
            let window = &ps[i - LUMP_HALF_WINDOW..=i + LUMP_HALF_WINDOW];
            let mean = window.iter().map(|r| r.1).sum::<f64>() / window.len() as f64;
            // Lump: height above the 40 m moving mean, in mm.
            let lump = (ps[i].1 - mean) * 1000.0;
            let t = turn(ps, i, 4);
            if t < 5.0 {
                lump_straight.push(lump);
            } else if t > 10.0 {
                lump_curve.push(lump);
            }
        }
    }

    let max = |v: &[f64]| v.iter().cloned().fold(0.0, f64::max);
    let steps_over_5cm = steps.iter().filter(|s| s.0 > 0.05).count();
    let step_max = steps.iter().map(|s| s.0).fold(0.0, f64::max);
    println!(
        "ribbon {id}: {} runs, {} steps within {HOME_RADIUS_M} m of home",
        samples.len(),
        steps.len()
    );
    println!(
        "pitch straight: max {:.2} deg, p95 {:.2} deg ({} steps)",
        max(&pitch_straight),
        ribbon_percentile(&mut pitch_straight.clone(), 0.95),
        pitch_straight.len()
    );
    println!(
        "pitch curve: max {:.2} deg, p95 {:.2} deg ({} steps)",
        max(&pitch_curve),
        ribbon_percentile(&mut pitch_curve.clone(), 0.95),
        pitch_curve.len()
    );
    println!(
        "step (2nd difference): max {:.2} cm, p99 {:.2} cm, steps > 5 cm {} of {}",
        step_max * 100.0,
        ribbon_percentile(&mut steps.iter().map(|s| s.0).collect::<Vec<_>>(), 0.99) * 100.0,
        steps_over_5cm,
        steps.len()
    );
    println!(
        "lumps: straight std {:.1} mm ({} samples), curve std {:.1} mm ({} samples)",
        ribbon_std(&lump_straight),
        lump_straight.len(),
        ribbon_std(&lump_curve),
        lump_curve.len()
    );
    println!(
        "axis above sample_height: max {:.3} m at ({:.1}, {:.1}), {} samples without terrain",
        axis_above_terrain.0, axis_above_terrain.1[0], axis_above_terrain.1[1], axis_no_terrain
    );
    let mut border_steps: Vec<f64> = steps
        .iter()
        .filter(|s| border_gap(s.1) < 5.0)
        .map(|s| s.0)
        .collect();
    println!(
        "step p99 within 5 m of a tile border: {:.2} cm over {} steps (all steps p99 {:.2} cm)",
        ribbon_percentile(&mut border_steps, 0.99) * 100.0,
        border_steps.len(),
        ribbon_percentile(&mut steps.iter().map(|s| s.0).collect::<Vec<_>>(), 0.99) * 100.0
    );

    steps.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    println!("15 worst steps:");
    for (rank, (step, at, key)) in steps.iter().take(15).enumerate() {
        // The OSM way: the nearest road polyline of the tile the step belongs to.
        let tile = index.read_tile_at(key.0, key.1).expect("read tile");
        let mut way = (f64::INFINITY, String::from("?"));
        for road in &tile.roads {
            for seg in road.points.windows(2) {
                let (a, b) = (seg[0], seg[1]);
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let len2 = dx * dx + dy * dy;
                let t = if len2 > 0.0 {
                    (((at[0] - a[0]) * dx + (at[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let d = (at[0] - (a[0] + dx * t)).hypot(at[1] - (a[1] + dy * t));
                if d < way.0 {
                    way = (d, format!("{} '{}'", road.highway, road.name));
                }
            }
        }
        println!(
            "  {:2}. {:7.2} cm at ({:.1}, {:.1}) way {}",
            rank + 1,
            step * 100.0,
            at[0],
            at[1],
            way.1
        );
    }
}

/// Height of the ribbon triangle at `(x, y)` with a vertex nearest to `near` (`None` outside every
/// triangle). A ribbon can pass over itself or over another road; picking the nearest vertex keeps
/// the reading at the crossing instead of at a distant part of the same strip, and at a shared
/// node it picks the triangle that has the node vertex (distance zero).
fn ribbon_height_near(mesh: &MeshData, x: f64, y: f64, near: [f64; 2]) -> Option<f64> {
    let mut best: Option<(f64, f64)> = None;
    for tri in mesh.indices.chunks(3) {
        let p: Vec<[f64; 3]> = tri
            .iter()
            .map(|&i| {
                let v = mesh.vertices[i as usize];
                [v[0] as f64, v[1] as f64, -(v[2] as f64)]
            })
            .collect();
        let det =
            (p[1][0] - p[0][0]) * (p[2][2] - p[0][2]) - (p[2][0] - p[0][0]) * (p[1][2] - p[0][2]);
        if det.abs() < 1e-12 {
            continue;
        }
        let l1 = ((x - p[0][0]) * (p[2][2] - p[0][2]) - (p[2][0] - p[0][0]) * (y - p[0][2])) / det;
        let l2 = ((p[1][0] - p[0][0]) * (y - p[0][2]) - (x - p[0][0]) * (p[1][2] - p[0][2])) / det;
        let l0 = 1.0 - l1 - l2;
        let eps = -1e-9;
        if l0 < eps || l1 < eps || l2 < eps {
            continue;
        }
        let h = l0 * p[0][1] + l1 * p[1][1] + l2 * p[2][1];
        let d = p
            .iter()
            .map(|v| (v[0] - near[0]).hypot(v[2] - near[1]))
            .fold(f64::INFINITY, f64::min);
        if best.map_or(true, |(bd, _)| d < bd) {
            best = Some((d, h));
        }
    }
    best.map(|(_, h)| h)
}

/// Two roads that meet at a node on a slope: one climbs along the slope (it needs the
/// anti-poke-through lift), the other runs along the contour (it needs none). Drawn apart, the
/// climbing ribbon steps above the contour one by that lift; the junction weld gives both the
/// same height at the node.
#[test]
fn junction_ribbons_meet_at_one_height_on_a_slope() {
    let road = |points: Vec<[f64; 2]>| Polyline {
        oneway: None,
        lanes: None,
        maxspeed: None,
        roundabout: false,
        name: String::new(),
        highway: "residential".into(),
        points,
        z: Vec::new(),
        zl: Vec::new(),
        zr: Vec::new(),
        lit: None,
    };
    // 20% grade along x (0.5 m per 2.5 m station, well past `ROAD_STEP_FREE_M`).
    let t = tile(
        0,
        0,
        grid(26, |u, _| 100.0 + 0.2 * (u * 250.0) as f32),
        vec![
            // Climbs with the slope, through the shared node.
            road(vec![[80.0, 125.0], [125.0, 125.0], [170.0, 125.0]]),
            // Runs along the contour, through the same node.
            road(vec![[125.0, 80.0], [125.0, 125.0], [125.0, 170.0]]),
        ],
    );
    let at = [125.0, 125.0];
    let ground = |x: f64, y: f64| sample_height(&t, 250.0, x, y);
    let height =
        |mesh: &MeshData| ribbon_height_near(mesh, at[0], at[1], at).expect("ribbon at node");

    // Apart: the climbing ribbon sits above the contour one by its anti-poke-through lift.
    let apart = (
        height(&ribbon_of_road(&t, 250.0, &ground, &[], 0, &[])),
        height(&ribbon_of_road(&t, 250.0, &ground, &[], 1, &[])),
    );
    let apart_step = (apart.0 - apart.1).abs();
    assert!(
        apart_step > 0.02,
        "fixture must show the step before the junction weld (got {:.3} m)",
        apart_step
    );

    // At the junction the two ribbons meet at one height (only the 2 mm order lift remains).
    let together = (
        height(&ribbon_of_road(&t, 250.0, &ground, &[], 0, &[1])),
        height(&ribbon_of_road(&t, 250.0, &ground, &[], 1, &[0])),
    );
    let step = (together.0 - together.1).abs();
    assert!(
        step <= 0.02,
        "junction step {:.3} m > 0.02 m at the shared node",
        step
    );
}

/// Where the segments `a0 -> a1` and `b0 -> b1` cross in 2D (map meters), or `None` when they are
/// parallel or the crossing lies off either segment. Ends do not count: two ways that touch at a
/// node are a junction, found as a shared node, not as a crossing here.
fn segment_crossing(a0: [f64; 2], a1: [f64; 2], b0: [f64; 2], b1: [f64; 2]) -> Option<[f64; 2]> {
    let (rx, ry) = (a1[0] - a0[0], a1[1] - a0[1]);
    let (sx, sy) = (b1[0] - b0[0], b1[1] - b0[1]);
    let den = rx * sy - ry * sx;
    if den.abs() < 1e-9 {
        return None;
    }
    let (qx, qy) = (b0[0] - a0[0], b0[1] - a0[1]);
    let t = (qx * sy - qy * sx) / den;
    let u = (qx * ry - qy * rx) / den;
    const END_MARGIN: f64 = 0.02;
    let inside = |v: f64| v > END_MARGIN && v < 1.0 - END_MARGIN;
    (inside(t) && inside(u)).then(|| [a0[0] + rx * t, a0[1] + ry * t])
}

/// Ways a car can drive on. Footways, paths, steps and the other pedestrian ways make no crossing
/// that a wheel feels, so they are left out of the junction measurement.
fn drivable_way(highway: &str) -> bool {
    !matches!(
        highway,
        "footway"
            | "path"
            | "pedestrian"
            | "cycleway"
            | "steps"
            | "corridor"
            | "bridleway"
            | "platform"
            | "construction"
            | "proposed"
            | "raceway"
            | "bus_stop"
            | "elevator"
    )
}

/// Bounding box of a polyline as `[x0, y0, x1, y1]`.
fn points_bbox(points: &[[f64; 2]]) -> [f64; 4] {
    let mut b = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for p in points {
        b[0] = b[0].min(p[0]);
        b[1] = b[1].min(p[1]);
        b[2] = b[2].max(p[0]);
        b[3] = b[3].max(p[1]);
    }
    b
}

fn boxes_overlap(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

/// One place where two drivable ribbons meet or cross, with the grade of both ways there.
struct RibbonCrossing {
    at: [f64; 2],
    tile: (i32, i32),
    /// Road index in the tile and a label (`highway 'name'`) of each way.
    ways: [(usize, String); 2],
    /// Direction of each way at the crossing (unit, map meters).
    dirs: [[f64; 2]; 2],
    /// True when the bake pins a node shared by both ways: their profiles already meet there.
    shared: bool,
    /// Grade of each way at the crossing, in percent.
    grades: [f64; 2],
}

/// Grade in percent of the ground around `at` along `dir`, over `half` meters each way.
fn grade_percent(
    height: &dyn Fn(f64, f64) -> Option<f64>,
    at: [f64; 2],
    dir: [f64; 2],
    half: f64,
) -> f64 {
    let at_d = |s: f64| [at[0] + dir[0] * s, at[1] + dir[1] * s];
    let (Some(a), Some(b)) = (
        height(at_d(-half)[0], at_d(-half)[1]),
        height(at_d(half)[0], at_d(half)[1]),
    ) else {
        return 0.0;
    };
    ((b - a) / (2.0 * half) * 100.0).abs()
}

/// Junction step on the real baked map `casa-10km` (measurement, not a pass/fail test): every
/// place where two drivable ribbons meet at a shared node or cross without one, with the grade of
/// both ways there and the height step between the two drawn ribbons. Prints the crossings with a
/// grade above 10% sorted by step, and the worst step of the map either way.
/// Run: `cargo test --release -p driving_the_earth_ext junction_step -- --ignored --nocapture`.
#[test]
#[ignore]
fn junction_step_on_slopes() {
    use std::collections::{HashMap, HashSet};

    /// Grade above which a crossing counts as "on a slope".
    const GRADE_MIN: f64 = 10.0;
    /// Half-length of the stretch over which the grade at a crossing is read, in meters.
    const GRADE_HALF_M: f64 = 5.0;
    /// Two road points within this distance are the same node. Matches `field::NODE_KEY_M`: a
    /// looser key would count near-misses the bake never reconciles.
    const NODE_SAME_M: f64 = 0.1;
    /// Radius around the crossing point over which the worst step is read, in meters.
    const STEP_RADIUS_M: f64 = 3.0;

    let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../cache"));
    let index = crate::world::open_map(root, "casa-10km").expect("baked map 'casa-10km'");
    let tm = index.meta.tile_size_m;
    let map_grid = tilebake::dem::MapGrid::from_meta(&index.meta);

    // Pass 1: the crossings of each tile. Shared nodes come from the node keys of the ways (the
    // bake pins one height for them); geometric crossings are segment intersections away from any
    // node. The whole map is scanned once: a crossing near a tile border shows up in both tiles,
    // and the 1 m key keeps the first.
    let mut found: Vec<RibbonCrossing> = Vec::new();
    let mut seen: HashSet<(i64, i64)> = HashSet::new();
    let mut pairs = 0usize;
    for &(tx, ty) in index.coords() {
        let tile = index.read_tile_at(tx, ty).expect("read tile");
        // Road index in `tile.roads` (the ribbons index the full list; `lift` uses it too).
        let roads: Vec<(usize, &Polyline)> = tile
            .roads
            .iter()
            .enumerate()
            .filter(|(_, r)| drivable_way(&r.highway) && r.points.len() >= 2)
            .collect();
        // Grade at a crossing, read from this tile's baked grid (its edge plane matches the
        // neighbor's, so a crossing near a border reads the same).
        let ground_h = |x: f64, y: f64| Some(sample_height(&tile, tm, x, y) as f64);
        let grades = |at: [f64; 2], dirs: [[f64; 2]; 2]| {
            [
                grade_percent(&ground_h, at, dirs[0], GRADE_HALF_M),
                grade_percent(&ground_h, at, dirs[1], GRADE_HALF_M),
            ]
        };
        // Shared nodes: the ways with a point on each key, and the direction each leaves it.
        let mut nodes: HashMap<(i64, i64), Vec<(usize, usize, [f64; 2])>> = HashMap::new();
        for (i, (_, road)) in roads.iter().enumerate() {
            let n = road.points.len();
            for k in 0..n {
                let key = (
                    (road.points[k][0] / NODE_SAME_M).round() as i64,
                    (road.points[k][1] / NODE_SAME_M).round() as i64,
                );
                // Direction of the way at the node: from the previous point (or the next, at an end).
                let q = if k > 0 {
                    road.points[k - 1]
                } else {
                    road.points[k + 1]
                };
                let d = [road.points[k][0] - q[0], road.points[k][1] - q[1]];
                let len = d[0].hypot(d[1]);
                let dir = if len > 1e-9 {
                    [d[0] / len, d[1] / len]
                } else {
                    [1.0, 0.0]
                };
                nodes.entry(key).or_default().push((i, k, dir));
            }
        }
        for entries in nodes.values() {
            for a in 0..entries.len() {
                for b in a + 1..entries.len() {
                    let ((i, k, dir_a), (j, _, dir_b)) = (entries[a], entries[b]);
                    if i == j {
                        continue;
                    }
                    let at = roads[i].1.points[k];
                    let key = ((at[0] / 1.0).round() as i64, (at[1] / 1.0).round() as i64);
                    if !seen.insert(key) {
                        continue;
                    }
                    found.push(RibbonCrossing {
                        at,
                        tile: (tx, ty),
                        ways: [
                            (
                                roads[i].0,
                                format!("{} '{}'", roads[i].1.highway, roads[i].1.name),
                            ),
                            (
                                roads[j].0,
                                format!("{} '{}'", roads[j].1.highway, roads[j].1.name),
                            ),
                        ],
                        dirs: [dir_a, dir_b],
                        shared: true,
                        grades: grades(at, [dir_a, dir_b]),
                    });
                }
            }
        }
        // Geometric crossings: segments that cut each other away from every node.
        let boxes: Vec<[f64; 4]> = roads.iter().map(|r| points_bbox(&r.1.points)).collect();
        for i in 0..roads.len() {
            for j in i + 1..roads.len() {
                if !boxes_overlap(boxes[i], boxes[j]) {
                    continue;
                }
                pairs += 1;
                for wa in roads[i].1.points.windows(2) {
                    for wb in roads[j].1.points.windows(2) {
                        let Some(at) = segment_crossing(wa[0], wa[1], wb[0], wb[1]) else {
                            continue;
                        };
                        let key = ((at[0] / 1.0).round() as i64, (at[1] / 1.0).round() as i64);
                        if !seen.insert(key) {
                            continue;
                        }
                        let dir = |w: &[[f64; 2]]| {
                            let d = [w[1][0] - w[0][0], w[1][1] - w[0][1]];
                            let len = d[0].hypot(d[1]);
                            [d[0] / len, d[1] / len]
                        };
                        found.push(RibbonCrossing {
                            at,
                            tile: (tx, ty),
                            ways: [
                                (
                                    roads[i].0,
                                    format!("{} '{}'", roads[i].1.highway, roads[i].1.name),
                                ),
                                (
                                    roads[j].0,
                                    format!("{} '{}'", roads[j].1.highway, roads[j].1.name),
                                ),
                            ],
                            dirs: [dir(wa), dir(wb)],
                            shared: false,
                            grades: grades(at, [dir(wa), dir(wb)]),
                        });
                    }
                }
            }
        }
    }

    let grade_of = |c: &RibbonCrossing| c.grades[0].max(c.grades[1]);

    // Pass 2: the drawn step at the crossings on a slope, one ribbon at a time on the same field.
    let step_of = |c: &RibbonCrossing| -> Option<(f64, f64, f64)> {
        let map = index.neighborhood(c.tile.0, c.tile.1).ok()?;
        let tile = map
            .tiles
            .iter()
            .find(|t| (t.x, t.y) == c.tile)
            .expect("center tile");
        let around: Vec<&Tile> = map.tiles.iter().filter(|t| (t.x, t.y) != c.tile).collect();
        let ground = |x: f64, y: f64| {
            map.height_at(x, y)
                .unwrap_or_else(|| sample_height(tile, tm, x, y))
        };
        let lifts: Vec<f64> = (0..2)
            .map(|s| {
                let idx = c.ways[s].0;
                (ROAD_LIFT_M + (idx % 8) as f32 * ROAD_ORDER_LIFT_M) as f64
            })
            .collect();
        // Every other road with a point on the same node key, as the game reconciles them.
        let node_key = |p: [f64; 2]| ((p[0] / 0.1).round() as i64, (p[1] / 0.1).round() as i64);
        let at_key = node_key(c.at);
        let partners_near = |idx: usize| -> Vec<usize> {
            tile.roads
                .iter()
                .enumerate()
                .filter(|&(j, road)| j != idx && road.points.iter().any(|p| node_key(*p) == at_key))
                .map(|(j, _)| j)
                .collect()
        };
        let meshes: Vec<MeshData> = (0..2)
            .map(|s| {
                let idx = c.ways[s].0;
                ribbon_of_road(tile, tm, &ground, &around, idx, &partners_near(idx))
            })
            .collect();
        let at = c.at;
        let (Some(ha), Some(hb)) = (
            ribbon_height_near(&meshes[0], at[0], at[1], at),
            ribbon_height_near(&meshes[1], at[0], at[1], at),
        ) else {
            return None;
        };
        // Gap at the crossing point, and the worst gap in a small window around it (a wheel
        // crosses the band, not a point).
        let point = (ha - hb).abs();
        let mut worst = point;
        let n = (STEP_RADIUS_M / 0.75) as i32;
        for i in -n..=n {
            for j in -n..=n {
                let (x, y) = (at[0] + i as f64 * 0.75, at[1] + j as f64 * 0.75);
                if (x - at[0]).hypot(y - at[1]) > STEP_RADIUS_M {
                    continue;
                }
                if let (Some(pa), Some(pb)) = (
                    ribbon_height_near(&meshes[0], x, y, at),
                    ribbon_height_near(&meshes[1], x, y, at),
                ) {
                    worst = worst.max((pa - pb).abs());
                }
            }
        }
        let lift_signed = (lifts[0] - lifts[1]).abs();
        Some((point, worst, worst - lift_signed))
    };

    // (step at the crossing, worst step within the window, terrain-only worst, grade, crossing).
    let mut on_slope: Vec<(f64, f64, f64, f64, &RibbonCrossing)> = Vec::new();
    let mut no_mesh = 0usize;
    for c in &found {
        if grade_of(c) <= GRADE_MIN {
            continue;
        }
        match step_of(c) {
            Some((point, worst, terrain_step)) => {
                on_slope.push((point, worst, terrain_step, grade_of(c), c));
            }
            None => no_mesh += 1,
        }
    }
    on_slope.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let shared = found.iter().filter(|c| c.shared).count();

    println!(
        "casa-10km: {} crossings of two drivable ribbons ({} at a shared node, {} geometric), {} road pairs near each other",
        found.len(),
        shared,
        found.len() - shared,
        pairs
    );
    println!(
        "grade > {GRADE_MIN:.0}%: {} crossings ({} without a ribbon on the point), {} at a shared node",
        on_slope.len(),
        no_mesh,
        on_slope.iter().filter(|c| c.4.shared).count()
    );
    let steps: Vec<f64> = on_slope.iter().map(|c| c.0).collect();
    if !steps.is_empty() {
        let max = steps.iter().cloned().fold(0.0, f64::max);
        let over_2cm = steps.iter().filter(|s| **s > 0.02).count();
        let mut sorted = steps.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "step at the crossing on a slope: max {:.2} cm, {} crossings over 2 cm, median {:.2} cm, p90 {:.2} cm",
            max * 100.0,
            over_2cm,
            sorted[sorted.len() / 2] * 100.0,
            sorted[(sorted.len() - 1) * 9 / 10] * 100.0
        );
        let worsts: Vec<f64> = on_slope.iter().map(|c| c.1).collect();
        println!(
            "worst step within {STEP_RADIUS_M:.0} m of the crossing: max {:.2} cm, {} over 2 cm",
            worsts.iter().cloned().fold(0.0, f64::max) * 100.0,
            worsts.iter().filter(|s| **s > 0.02).count()
        );
        // Histogram of the step at the crossing, shared nodes and geometric crossings apart.
        let bins = [0.0, 0.01, 0.02, 0.05, 0.10, 0.20, 0.50];
        for (label, want_shared) in [("node", true), ("cross", false)] {
            let v: Vec<f64> = on_slope
                .iter()
                .filter(|c| c.4.shared == want_shared)
                .map(|c| c.0)
                .collect();
            if v.is_empty() {
                continue;
            }
            let buckets: Vec<usize> = (0..bins.len() - 1)
                .map(|k| {
                    v.iter()
                        .filter(|s| **s >= bins[k] && **s < bins[k + 1])
                        .count()
                })
                .collect();
            let over = v.iter().filter(|s| **s >= bins[bins.len() - 1]).count();
            println!(
                "  {label} step bins (cm) {}: {} ; >=50: {}",
                (0..bins.len() - 1)
                    .map(|k| format!("[{:.0},{:.0})", bins[k] * 100.0, bins[k + 1] * 100.0))
                    .collect::<Vec<_>>()
                    .join(" "),
                buckets
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(" "),
                over
            );
        }
    }
    // Shared nodes still over the limit.
    let outliers: Vec<&(f64, f64, f64, f64, &RibbonCrossing)> = on_slope
        .iter()
        .filter(|c| c.4.shared && c.0 > 0.02)
        .collect();
    if !outliers.is_empty() {
        println!("shared nodes over 2 cm ({}):", outliers.len());
        for &(step, _, _, grade, c) in outliers.iter().copied().take(20) {
            let (lat, lon) = map_grid.latlon(c.at[0], c.at[1]);
            let mut extra = String::new();
            if let Ok(t) = index.read_tile_at(c.tile.0, c.tile.1) {
                let nearest = |idx: usize| {
                    t.roads[idx]
                        .points
                        .iter()
                        .copied()
                        .min_by(|p, q| {
                            let dp = (p[0] - c.at[0]).hypot(p[1] - c.at[1]);
                            let dq = (q[0] - c.at[0]).hypot(q[1] - c.at[1]);
                            dp.partial_cmp(&dq).unwrap()
                        })
                        .unwrap_or(c.at)
                };
                let (pa, pb) = (nearest(c.ways[0].0), nearest(c.ways[1].0));
                let sep = (pa[0] - pb[0]).hypot(pa[1] - pb[1]);
                extra = format!(
                    " | node gap {:.3} m, profile points {}/{}",
                    sep,
                    t.roads[c.ways[0].0].z.len(),
                    t.roads[c.ways[1].0].z.len()
                );
            }
            println!(
                "  step {:.2} cm, grade {:.1}%, tile ({},{}), {:.5},{:.5}: {} / {}{}",
                step * 100.0,
                grade,
                c.tile.0,
                c.tile.1,
                lat,
                lon,
                c.ways[0].1,
                c.ways[1].1,
                extra,
            );
        }
    }
    // Worst shared-node junctions, which is where a "junction" mesh applies.
    println!("worst shared nodes (junction step):");
    for (rank, (step, _, _, grade, c)) in
        on_slope.iter().filter(|c| c.4.shared).take(10).enumerate()
    {
        let (lat, lon) = map_grid.latlon(c.at[0], c.at[1]);
        println!(
            " {:2}  {:6.2} cm  grade {:5.1}%  tile ({:3},{:3})  ({:9.1},{:9.1})  {:.5},{:.5}  {} / {}",
            rank + 1,
            step * 100.0,
            grade,
            c.tile.0,
            c.tile.1,
            c.at[0],
            c.at[1],
            lat,
            lon,
            c.ways[0].1,
            c.ways[1].1,
        );
    }
    println!(
        "  #  step cm  win cm  terr cm  grade%  angle  kind   tile        x,y          lat,lon                 A / B"
    );
    for (rank, (step, worst, terrain_step, grade, c)) in on_slope.iter().enumerate().take(40) {
        let (lat, lon) = map_grid.latlon(c.at[0], c.at[1]);
        let dot = (c.dirs[0][0] * c.dirs[1][0] + c.dirs[0][1] * c.dirs[1][1]).abs();
        let angle = dot.clamp(0.0, 1.0).acos().to_degrees();
        println!(
            " {:2}  {:7.2}  {:6.2}  {:7.2}  {:6.1}  {:5.0}  {:5}  ({:3},{:3})  ({:8.1},{:8.1})  {:.5},{:.5}  {} / {}",
            rank + 1,
            step * 100.0,
            worst * 100.0,
            terrain_step * 100.0,
            grade,
            angle,
            if c.shared { "node" } else { "cross" },
            c.tile.0,
            c.tile.1,
            c.at[0],
            c.at[1],
            lat,
            lon,
            c.ways[0].1,
            c.ways[1].1,
        );
    }
    if on_slope.len() > 40 {
        println!("... {} more", on_slope.len() - 40);
    }
}

#[test]
fn collision_under_a_profiled_road_follows_the_profile() {
    // Flat ground at 50 m, a road with a flat profile at 60 m: its ribbon sits at 60 m, so the
    // collision heightfield must too, on the axis. Far off the road it keeps the recorded 50 m.
    let road = Polyline {
        oneway: None,
        lanes: None,
        maxspeed: None,
        roundabout: false,
        name: String::new(),
        highway: "residential".into(),
        points: vec![[50.0, 125.0], [200.0, 125.0]],
        z: vec![60.0, 60.0],
        zl: Vec::new(),
        zr: Vec::new(),
        lit: None,
    };
    let t = tile(0, 0, grid(26, |_, _| 50.0), vec![road]);
    let g = tile_geometry(&t, 250.0);
    let n = TERRAIN_FINE_N;
    let at =
        |x: f64, y: f64| g.collision[(y / 2.5).round() as usize * n + (x / 2.5).round() as usize];
    for x in [60.0, 100.0, 150.0, 190.0] {
        let h = at(x, 125.0) as f64;
        let ribbon = ribbon_height(&g.roads, x, 125.0).expect("road covers the axis");
        assert!(
            (h - 60.0).abs() < 0.05,
            "collision {h} at x={x}, profile 60"
        );
        assert!(
            (h - (ribbon - 0.1)).abs() < 0.05,
            "collision {h} vs ribbon {ribbon} at x={x}"
        );
    }
    let off = at(100.0, 240.0) as f64;
    assert!(
        (off - 50.0).abs() < 0.05,
        "off the road the recorded ground stays: {off}"
    );
}

#[test]
fn collision_grid_matches_a_surface_on_the_recorded_ground() {
    // The fine mesh's collision (its interior plus the edge fallback) must equal the surface built on
    // the fine grid with the recorded ground: the same heights at every point of the collision grid.
    let profiled = |points: Vec<[f64; 2]>, z: Vec<f32>| Polyline {
        oneway: None,
        lanes: None,
        maxspeed: None,
        roundabout: false,
        name: String::new(),
        highway: "residential".into(),
        points,
        z,
        zl: Vec::new(),
        zr: Vec::new(),
        lit: None,
    };
    let t0 = tile(
        0,
        0,
        grid(26, |u, v| hill(u * 250.0, v * 250.0)),
        vec![
            profiled(vec![[30.0, 60.0], [240.0, 150.0]], vec![55.0, 62.0]),
            profiled(vec![[120.0, 0.0], [120.0, 250.0]], vec![48.0, 52.0]),
        ],
    );
    let t1 = tile(
        1,
        0,
        grid(26, |u, v| hill(250.0 + u * 250.0, v * 250.0)),
        vec![profiled(
            vec![[260.0, 170.0], [400.0, 120.0]],
            vec![60.0, 58.0],
        )],
    );
    let g = tile_geometry_with(&t0, 250.0, &|x, y| sample_height(&t0, 250.0, x, y), &[&t1]);
    let field = Field::new(
        [&t0, &t1].into_iter().flat_map(|t| t.roads.iter()),
        [-250.0, -250.0],
    );
    let reference = collision_heights(&t0, 250.0, Some(field));
    assert_eq!(g.collision.len(), reference.len());
    let worst = g
        .collision
        .iter()
        .zip(&reference)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        worst < 1e-3,
        "collision differs from the reference by {worst} m"
    );
}
