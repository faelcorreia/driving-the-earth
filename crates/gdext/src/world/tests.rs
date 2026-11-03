//! World tests: map index, `Map` height lookup, loading from disk and tile seams across borders.
//! Synthetic tiles come from `geometry::tests`. The real-map seam test is ignored by default.
use std::fs;
use std::io;
use std::path::Path;

use tileformat::{write_tile, MapMeta, Polyline};

use super::map::Map;
use super::*;
use crate::geometry::tests::{grid, ribbon_height, tile};
use crate::geometry::{road_width_m, tile_geometry, ROAD_LIFT_M};

fn meta(complete: bool) -> MapMeta {
    MapMeta {
        id: "t".into(),
        name: "t".into(),
        center_lat: 0.0,
        center_lon: 0.0,
        radius_m: 1000.0,
        tile_size_m: 20.0,
        created_unix: 0,
        complete,
    }
}

/// Reads every tile of a map eagerly (tests only).
fn load_map(root: &Path, id: &str) -> io::Result<Map> {
    let index = open_map(root, id)?;
    let tiles = index
        .coords()
        .iter()
        .map(|&(x, y)| index.read_tile_at(x, y))
        .collect::<io::Result<Vec<_>>>()?;
    Ok(Map::new(index.meta.clone(), tiles))
}

fn temp_cache_root(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("gdext-map-{tag}-{}-{nanos}", std::process::id()))
}

#[test]
fn height_at_finds_the_tile_and_none_outside() {
    let a = tile(0, 0, grid(3, |_, _| 10.0), Vec::new());
    let b = tile(-1, 0, grid(3, |_, _| 20.0), Vec::new());
    let map = Map::new(meta(true), vec![b, a]);
    assert_eq!(map.height_at(0.0, 0.0), Some(10.0));
    assert_eq!(map.height_at(-0.5, 5.0), Some(20.0));
    assert_eq!(map.height_at(25.0, 0.0), None);
    assert_eq!(map.height_at(0.0, -1.0), None);
}

#[test]
fn load_map_reads_meta_and_tiles() {
    let root = temp_cache_root("ok");
    let dir = root.join("t");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("meta.json"),
        serde_json::to_string(&meta(true)).unwrap(),
    )
    .unwrap();
    write_tile(&root, "t", &tile(1, 0, grid(3, |_, _| 2.0), Vec::new())).unwrap();
    write_tile(&root, "t", &tile(0, 0, grid(3, |_, _| 1.0), Vec::new())).unwrap();
    fs::write(dir.join("tiles").join("notes.txt"), "ignored").unwrap();

    let map = load_map(&root, "t").unwrap();
    assert_eq!(
        map.tiles.iter().map(|t| (t.x, t.y)).collect::<Vec<_>>(),
        vec![(0, 0), (1, 0)]
    );
    assert_eq!(map.height_at(0.0, 0.0), Some(1.0));
    assert_eq!(map.height_at(30.0, 0.0), Some(2.0));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn load_map_rejects_incomplete_map() {
    let root = temp_cache_root("incomplete");
    let dir = root.join("t");
    fs::create_dir_all(dir.join("tiles")).unwrap();
    fs::write(
        dir.join("meta.json"),
        serde_json::to_string(&meta(false)).unwrap(),
    )
    .unwrap();
    assert!(load_map(&root, "t").is_err());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn neighborhood_geometry_matches_whole_map() {
    let root = temp_cache_root("nbhd");
    let dir = root.join("t");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("meta.json"),
        serde_json::to_string(&meta(true)).unwrap(),
    )
    .unwrap();
    let road = |points: Vec<[f64; 2]>| Polyline {
        name: String::new(),
        z: Vec::new(),
        zl: Vec::new(),
        zr: Vec::new(),
        highway: "residential".into(),
        points,
    };
    // A road crosses the border between (0, 0) and (1, 0); the others are sloped.
    write_tile(
        &root,
        "t",
        &tile(
            0,
            0,
            grid(3, |u, _| (3.0 * u) as f32),
            vec![road(vec![[15.0, 10.0], [25.0, 10.0]])],
        ),
    )
    .unwrap();
    write_tile(
        &root,
        "t",
        &tile(
            1,
            0,
            grid(3, |u, v| (20.0 + 4.0 * v + u) as f32),
            vec![road(vec![[22.0, 5.0], [33.0, 15.0]])],
        ),
    )
    .unwrap();
    write_tile(
        &root,
        "t",
        &tile(0, 1, grid(3, |_, v| (5.0 * v) as f32), Vec::new()),
    )
    .unwrap();

    let whole = load_map(&root, "t").unwrap();
    let index = open_map(&root, "t").unwrap();
    for tile in &whole.tiles {
        let from_index = index.tile_geometry(tile.x, tile.y).unwrap();
        assert_eq!(
            from_index,
            whole.tile_geometry(tile),
            "tile {} {}",
            tile.x,
            tile.y
        );
    }
    fs::remove_dir_all(&root).unwrap();
}

/// Diagnostic on the real baked map (run with `--ignored --nocapture`): along every tile
/// edge, compares whether the road ribbon covers the point on each side, and its height.
#[test]
#[ignore]
fn real_map_road_seams() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache");
    let t0 = std::time::Instant::now();
    let map = load_map(&root, "casa").expect("baked map 'casa'");
    println!("load_map_ms={}", t0.elapsed().as_millis());
    let m = map.meta.tile_size_m;
    let t0 = std::time::Instant::now();
    let geo: Vec<_> = map
        .tiles
        .iter()
        .map(|t| (t.x, t.y, tile_geometry(t, m)))
        .collect();
    println!(
        "tile_geometry_ms={} tiles={}",
        t0.elapsed().as_millis(),
        geo.len()
    );
    let road_tris: usize = geo.iter().map(|g| g.2.roads.indices.len() / 3).sum();
    println!("road_triangles={road_tris}");
    let find = |x: i32, y: i32| geo.iter().find(|g| g.0 == x && g.1 == y).map(|g| &g.2);
    let roads: Vec<(f64, &Polyline)> = map
        .tiles
        .iter()
        .flat_map(|t| t.roads.iter())
        .map(|r| (road_width_m(&r.highway) / 2.0, r))
        .collect();
    let truth = |x: f64, y: f64| {
        roads.iter().any(|(half, r)| {
            r.points.windows(2).any(|w| {
                let (dx, dy) = (w[1][0] - w[0][0], w[1][1] - w[0][1]);
                let l2 = dx * dx + dy * dy;
                let t = if l2 > 0.0 {
                    (((x - w[0][0]) * dx + (y - w[0][1]) * dy) / l2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                ((x - w[0][0] - t * dx).hypot(y - w[0][1] - t * dy)) < *half - 0.1
            })
        })
    };
    let (mut gaps, mut height_diff, mut checked) = (0, 0, 0);
    for (tx, ty, a) in &geo {
        for (dx, dy) in [(1, 0), (0, 1)] {
            let Some(b) = find(tx + dx, ty + dy) else {
                continue;
            };
            let n = (m / 0.25) as usize;
            for k in 0..=n {
                let s = k as f64 * 0.25;
                // Near a tile corner the diagonal tile may draw the road; not checked here.
                let corner = s < 4.0 || s > m - 4.0;
                let (ex, ey) = if dx == 1 {
                    ((*tx + 1) as f64 * m, *ty as f64 * m + s)
                } else {
                    (*tx as f64 * m + s, (*ty + 1) as f64 * m)
                };
                let (ax, ay, bx, by) = if dx == 1 {
                    (ex - 1e-4, ey, ex + 1e-4, ey)
                } else {
                    (ex, ey - 1e-4, ex, ey + 1e-4)
                };
                let (ha, hb) = (
                    ribbon_height(&a.roads, ax, ay),
                    ribbon_height(&b.roads, bx, by),
                );
                checked += 1;
                let any_tile = |x: f64, y: f64| {
                    geo.iter()
                        .any(|g| ribbon_height(&g.2.roads, x, y).is_some())
                };
                let covered = if corner {
                    any_tile(ex - 1e-4, ey - 1e-4)
                        || any_tile(ex + 1e-4, ey + 1e-4)
                        || any_tile(ex - 1e-4, ey + 1e-4)
                        || any_tile(ex + 1e-4, ey - 1e-4)
                } else {
                    ha.is_some() || hb.is_some()
                };
                if truth(ex, ey) && !covered {
                    gaps += 1;
                    if gaps <= 20 {
                        println!("GAP: road covers edge point ({ex:.2}, {ey:.2}) but neither tile draws it");
                    }
                }
                if let (Some(u), Some(v)) = (ha, hb) {
                    if (u - v).abs() > 0.02 {
                        height_diff += 1;
                        if height_diff <= 10 {
                            println!("height differs at ({ex:.2}, {ey:.2}): {u:.3} vs {v:.3}");
                        }
                    }
                }
            }
        }
    }
    println!("checked={checked} gaps={gaps} height_diff={height_diff}");
}

#[test]
fn road_over_a_tile_border_follows_the_neighbor_terrain() {
    // Tile (0,0) is flat at 10 m; tile (1,0) is flat at 20 m. A road along y = 10 ends
    // at x = 20 + 3 (it crosses the border), so its tail lies over the neighbor.
    let road = Polyline {
        name: String::new(),
        z: Vec::new(),
        zl: Vec::new(),
        zr: Vec::new(),
        highway: "residential".into(),
        points: vec![[10.0, 10.0], [23.0, 10.0]],
    };
    let a = tile(0, 0, grid(3, |_, _| 10.0), vec![road]);
    let b = tile(1, 0, grid(3, |_, _| 20.0), Vec::new());
    let map = Map::new(meta(true), vec![a.clone(), b]);
    let g = map.tile_geometry(&a);
    let h = ribbon_height(&g.roads, 22.0, 10.0).unwrap();
    assert!((h - (20.0 + ROAD_LIFT_M as f64)).abs() < 0.05, "h = {h}");
}
