//! Distant lights (L2) bake: aggregates the building centroids of the baked tiles (real and generated)
//! into cells of `FAR_LIGHT_CELL_M` and writes one light per occupied cell to
//! `cache/<id>/far_lights.bin` (layout in `tileformat::far_lights`).
//!
//! Only cells whose center lies between `FAR_LIGHT_NEAR_M` and the map radius are kept: nearer ones
//! are drawn by the real buildings and lamps. The cells are global (not per tile), so a cell that
//! straddles a tile edge gives one light, not two. Position is the mean of the centroids in the
//! cell; height is the mean of each centroid sampled on its own tile's terrain.
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use tileformat::far_lights::{write_far_lights, FarLight};
use tileformat::terrain::sample_height;
use tileformat::units::{FAR_LIGHT_CELL_M, FAR_LIGHT_NEAR_M, TILE_M};
use tileformat::{read_tile, MapMeta, Tile};

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Summary {
    pub tiles: usize,
    pub buildings: usize,
    pub generated: usize,
    /// Cells with at least one building, before the distance filter.
    pub cells: usize,
    /// Lights written.
    pub points: usize,
}

#[derive(Default, Clone, Copy)]
struct Cell {
    count: u32,
    sum_x: f64,
    sum_y: f64,
    sum_z: f64,
}

/// Deterministic hue (0..=255) of a cell: a hash of its index, so the same cell keeps its color.
pub fn hue_of(ix: i32, iy: i32) -> u8 {
    let mut h = (ix as u32).wrapping_mul(0x9E37_79B1) ^ (iy as u32).wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 13;
    (h >> 24) as u8
}

/// Vertex mean of a polygon ring.
fn centroid(points: &[[f64; 2]]) -> Option<[f64; 2]> {
    if points.is_empty() {
        return None;
    }
    let n = points.len() as f64;
    let (sx, sy) = points
        .iter()
        .fold((0.0, 0.0), |(a, b), p| (a + p[0], b + p[1]));
    Some([sx / n, sy / n])
}

/// Pure aggregation: centroids of every building of `tiles` into cells, then the lights between the
/// near limit and `radius_m`. Also returns the counts for the summary.
pub fn aggregate<'a>(
    tiles: impl IntoIterator<Item = &'a Tile>,
    radius_m: f64,
) -> (Vec<FarLight>, Summary) {
    let mut cells: BTreeMap<(i32, i32), Cell> = BTreeMap::new();
    let mut summary = Summary::default();
    for tile in tiles {
        summary.tiles += 1;
        for b in &tile.buildings {
            let Some([cx, cy]) = centroid(&b.points) else {
                continue;
            };
            summary.buildings += 1;
            summary.generated += usize::from(b.generated);
            let key = (
                (cx / FAR_LIGHT_CELL_M).floor() as i32,
                (cy / FAR_LIGHT_CELL_M).floor() as i32,
            );
            let c = cells.entry(key).or_default();
            c.count += 1;
            c.sum_x += cx;
            c.sum_y += cy;
            c.sum_z += sample_height(tile, TILE_M, cx, cy) as f64;
        }
    }
    summary.cells = cells.len();
    let mut lights = Vec::new();
    for ((ix, iy), c) in cells {
        let n = c.count as f64;
        let (x, y) = (c.sum_x / n, c.sum_y / n);
        let r = x.hypot(y);
        if r < FAR_LIGHT_NEAR_M || r > radius_m {
            continue;
        }
        lights.push(FarLight {
            x: x as f32,
            y: y as f32,
            z: (c.sum_z / n) as f32,
            weight: c.count.min(255) as u8,
            hue: hue_of(ix, iy),
        });
    }
    summary.points = lights.len();
    (lights, summary)
}

/// Reads `cache/<id>/meta.json` and `cache/<id>/tiles/*.json`, and writes `cache/<id>/far_lights.bin`.
/// Reads only; the tiles are not rebaked.
pub fn bake_far_lights(cache_root: &Path, id: &str) -> io::Result<Summary> {
    let dir = cache_root.join(id);
    let meta: MapMeta = serde_json::from_slice(&std::fs::read(dir.join("meta.json"))?)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut paths: Vec<_> = std::fs::read_dir(dir.join("tiles"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    let tiles: Vec<Tile> = paths
        .iter()
        .map(|p| read_tile(p))
        .collect::<io::Result<_>>()?;
    let (lights, summary) = aggregate(&tiles, meta.radius_m);
    write_far_lights(&dir.join("far_lights.bin"), &lights)?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tileformat::{HeightGrid, Polygon};

    fn flat_tile(x: i32, y: i32, h: f32) -> Tile {
        Tile {
            signs: Vec::new(),
            x,
            y,
            height: HeightGrid {
                cols: 2,
                rows: 2,
                values: vec![h; 4],
            },
            roads: vec![],
            buildings: vec![],
            ground: vec![],
            fine_height: None,
        }
    }

    fn square(cx: f64, cy: f64, generated: bool) -> Polygon {
        let mut p = Polygon::real(vec![
            [cx - 3.0, cy - 3.0],
            [cx + 3.0, cy - 3.0],
            [cx + 3.0, cy + 3.0],
            [cx - 3.0, cy + 3.0],
        ]);
        p.generated = generated;
        p
    }

    #[test]
    fn buildings_in_one_cell_make_one_light_with_their_count() {
        let mut t = flat_tile(4, 0, 120.0);
        // Three centroids inside the cell [1000, 1020) x [1000, 1020) (on the 20 m grid, away from its edges).
        t.buildings = vec![
            square(1005.0, 1005.0, false),
            square(1010.0, 1012.0, true),
            square(1015.0, 1008.0, false),
        ];
        let (lights, s) = aggregate([&t], 10_000.0);
        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].weight, 3);
        assert_eq!(s.buildings, 3);
        assert_eq!(s.generated, 1);
        assert_eq!(s.cells, 1);
        let x = (1005.0 + 1010.0 + 1015.0) / 3.0;
        assert!((lights[0].x as f64 - x).abs() < 1e-3);
        assert!((lights[0].z - 120.0).abs() < 1e-4);
    }

    #[test]
    fn different_cells_make_different_lights_and_weight_saturates() {
        let mut t = flat_tile(4, 0, 50.0);
        t.buildings = (0..300).map(|_| square(1005.0, 1005.0, true)).collect();
        t.buildings.push(square(1045.0, 1005.0, false));
        let (lights, _) = aggregate([&t], 10_000.0);
        assert_eq!(lights.len(), 2);
        let w: Vec<u8> = lights.iter().map(|l| l.weight).collect();
        assert!(w.contains(&255) && w.contains(&1));
    }

    #[test]
    fn distance_filter_keeps_only_between_near_limit_and_radius() {
        let mut near = flat_tile(0, 0, 0.0);
        near.buildings = vec![square(100.0, 100.0, false)];
        let mut mid = flat_tile(3, 3, 0.0);
        mid.buildings = vec![square(900.0, 900.0, false)];
        let mut far = flat_tile(20, 0, 0.0);
        far.buildings = vec![square(5000.0, 0.0, false)];
        let (lights, s) = aggregate([&near, &mid, &far], 4000.0);
        assert_eq!(s.cells, 3);
        assert_eq!(lights.len(), 1);
        assert!(lights[0].x > 800.0);
    }

    #[test]
    fn hue_is_deterministic_and_spread() {
        assert_eq!(hue_of(12, -7), hue_of(12, -7));
        let distinct: std::collections::BTreeSet<u8> = (0..64).map(|i| hue_of(i, 3 * i)).collect();
        assert!(distinct.len() > 40);
    }
}
