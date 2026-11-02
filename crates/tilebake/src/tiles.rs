//! Raw data (OSM + DEM already in cache) -> tiles in `cache/<id>/tiles/`.
use std::fs;
use std::io;
use std::path::Path;

use serde_json::Value;
use tileformat::{MapMeta, Polygon, Polyline, Tile};

use crate::clip::{clip_polygon, clip_polyline, Rect};
use crate::dem::{download_dem, read_smoothed_dem, tile_heights, MapGrid, Raster};

fn invalid_data<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

/// Result of `bake_tiles`.
#[derive(Debug)]
pub struct BakeSummary {
    pub tiles: usize,
    /// Road pieces written over all tiles (a way crossing a border gives one piece per tile).
    pub road_pieces: usize,
    pub building_pieces: usize,
}

/// Tile `(x, y)` rectangle on the local plane.
pub fn tile_rect(x: i32, y: i32, tile_m: f64) -> Rect {
    Rect {
        x0: x as f64 * tile_m,
        y0: y as f64 * tile_m,
        x1: (x + 1) as f64 * tile_m,
        y1: (y + 1) as f64 * tile_m,
    }
}

/// Distance from 0 to the interval `[lo, hi]` (0 when it contains 0).
fn gap(lo: f64, hi: f64) -> f64 {
    if lo > 0.0 {
        lo
    } else if hi < 0.0 {
        -hi
    } else {
        0.0
    }
}

/// Tiles whose square intersects the disc of `radius_m` around the origin (the home).
/// Tiles that only touch the circle at one point are left out.
pub fn covering_tiles(radius_m: f64, tile_m: f64) -> Vec<(i32, i32)> {
    let n = (radius_m / tile_m).ceil() as i32 + 1;
    let r2 = radius_m * radius_m;
    let mut out = Vec::new();
    for x in -n..=n {
        for y in -n..=n {
            let dx = gap(x as f64 * tile_m, (x + 1) as f64 * tile_m);
            let dy = gap(y as f64 * tile_m, (y + 1) as f64 * tile_m);
            if dx * dx + dy * dy < r2 {
                out.push((x, y));
            }
        }
    }
    out
}

/// Roads and buildings from a raw Overpass response (`out body geom`), in the local plane.
pub fn parse_ways(overpass_json: &str, grid: &MapGrid) -> io::Result<(Vec<Polyline>, Vec<Polygon>)> {
    let v: Value = serde_json::from_str(overpass_json).map_err(invalid_data)?;
    let elements = v["elements"]
        .as_array()
        .ok_or_else(|| invalid_data("Overpass response has no 'elements'"))?;
    let mut roads = Vec::new();
    let mut buildings = Vec::new();
    for e in elements {
        let Some(geom) = e["geometry"].as_array() else {
            continue;
        };
        let points: Vec<[f64; 2]> = geom
            .iter()
            .filter_map(|p| {
                let (x, y) = grid.local(p["lat"].as_f64()?, p["lon"].as_f64()?);
                Some([x, y])
            })
            .collect();
        if let Some(highway) = e["tags"]["highway"].as_str() {
            if points.len() >= 2 {
                roads.push(Polyline {
                    highway: highway.to_string(),
                    points: points.clone(),
                });
            }
        }
        if e["tags"]["building"].is_string() {
            let mut ring = points;
            if ring.len() > 1 && ring.first() == ring.last() {
                ring.pop();
            }
            if ring.len() >= 3 {
                buildings.push(Polygon { points: ring });
            }
        }
    }
    Ok((roads, buildings))
}

/// Builds tile `(x, y)`: DEM grid plus the parts of the roads and buildings inside it.
/// Returns the tile and the number of road and building pieces.
pub fn build_tile(
    raster: &Raster,
    grid: &MapGrid,
    roads: &[Polyline],
    buildings: &[Polygon],
    x: i32,
    y: i32,
) -> io::Result<(Tile, usize, usize)> {
    let rect = tile_rect(x, y, grid.tile_m);
    let height = tile_heights(raster, grid, x, y)?;
    let mut tile_roads = Vec::new();
    for road in roads {
        for points in clip_polyline(&road.points, rect) {
            tile_roads.push(Polyline {
                highway: road.highway.clone(),
                points,
            });
        }
    }
    let mut tile_buildings = Vec::new();
    for b in buildings {
        let points = clip_polygon(&b.points, rect);
        if points.len() >= 3 {
            tile_buildings.push(Polygon { points });
        }
    }
    let counts = (tile_roads.len(), tile_buildings.len());
    let tile = Tile {
        x,
        y,
        height,
        roads: tile_roads,
        buildings: tile_buildings,
    };
    Ok((tile, counts.0, counts.1))
}

/// Bakes all the tiles covering the radius from the raw data in `cache_root/<id>/raw/`,
/// writes them to `cache_root/<id>/tiles/`, and then sets `meta.complete = true`.
/// Needs `meta.json` and `raw/overpass.json`; downloads the DEM tile only if it is missing.
pub fn bake_tiles(cache_root: &Path, id: &str) -> io::Result<BakeSummary> {
    let dir = cache_root.join(id);
    let meta_path = dir.join("meta.json");
    let mut meta: MapMeta = serde_json::from_str(&fs::read_to_string(&meta_path)?).map_err(invalid_data)?;
    let grid = MapGrid::from_meta(&meta);

    let overpass = fs::read_to_string(dir.join("raw").join("overpass.json"))?;
    let (roads, buildings) = parse_ways(&overpass, &grid)?;

    let (tif, _) = download_dem(cache_root, id, meta.center_lat, meta.center_lon)?;
    let raster = read_smoothed_dem(&tif)?;

    let mut summary = BakeSummary {
        tiles: 0,
        road_pieces: 0,
        building_pieces: 0,
    };
    for (x, y) in covering_tiles(meta.radius_m, meta.tile_size_m) {
        let (tile, r, b) = build_tile(&raster, &grid, &roads, &buildings, x, y)?;
        tileformat::write_tile(cache_root, id, &tile)?;
        summary.tiles += 1;
        summary.road_pieces += r;
        summary.building_pieces += b;
    }

    meta.complete = true;
    fs::write(&meta_path, serde_json::to_string_pretty(&meta).map_err(invalid_data)?)?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dem::CELL_M;

    #[test]
    fn covering_tiles_for_one_km_radius() {
        let tiles = covering_tiles(1000.0, 250.0);
        assert!(tiles.contains(&(0, 0)));
        assert!(tiles.contains(&(-4, 0)));
        assert!(tiles.contains(&(3, 0)));
        // Tile (4, 0) starts at x = 1000 m: only touches the circle, so it is out.
        assert!(!tiles.contains(&(4, 0)));
        // Corner tile (3, 3) is 750 m away on both axes: outside the disc.
        assert!(!tiles.contains(&(3, 3)));
        assert!(!tiles.contains(&(-4, -4)));
        assert_eq!(tiles.len(), 60);
    }

    #[test]
    fn tile_rect_and_neighbors_share_edge() {
        let a = tile_rect(0, 0, 250.0);
        let b = tile_rect(1, 0, 250.0);
        assert_eq!(a.x1, b.x0);
        assert_eq!((a.x0, a.y0, a.x1, a.y1), (0.0, 0.0, 250.0, 250.0));
    }

    fn flat_raster(grid: &MapGrid) -> Raster {
        // Constant 100 m DEM covering the whole map with margin.
        let step = 0.0001;
        let (w, h) = (400usize, 400usize);
        Raster {
            width: w,
            height: h,
            lon0: grid.lon0 - 0.02,
            lat0: grid.lat0 + 0.02,
            step_lon: step,
            step_lat: step,
            values: vec![100.0; w * h],
        }
    }

    const OSM: &str = r#"{"elements":[
        {"type":"way","id":1,"tags":{"highway":"residential"},
         "geometry":[{"lat":-19.9,"lon":-43.9},{"lat":-19.9,"lon":-43.89}]},
        {"type":"way","id":2,"tags":{"building":"yes"},
         "geometry":[{"lat":-19.9,"lon":-43.9},{"lat":-19.9,"lon":-43.899},
                     {"lat":-19.8995,"lon":-43.899},{"lat":-19.8995,"lon":-43.9},{"lat":-19.9,"lon":-43.9}]},
        {"type":"relation","id":3,"tags":{"type":"route"}}
    ]}"#;

    #[test]
    fn parse_ways_projects_and_closes_rings() {
        let grid = MapGrid {
            lat0: -19.9,
            lon0: -43.9,
            tile_m: 250.0,
            cell_m: CELL_M,
        };
        let (roads, buildings) = parse_ways(OSM, &grid).unwrap();
        assert_eq!(roads.len(), 1);
        assert_eq!(roads[0].highway, "residential");
        assert!(roads[0].points[0][0].abs() < 1e-6 && roads[0].points[0][1].abs() < 1e-6);
        // 0.01 degree of longitude at lat -19.9 is about 1045.6 m.
        assert!((roads[0].points[1][0] - 1045.6).abs() < 1.0, "{:?}", roads[0].points);
        assert_eq!(buildings.len(), 1);
        // Closing point removed: 4 corners.
        assert_eq!(buildings[0].points.len(), 4);
    }

    #[test]
    fn build_tile_clips_geometry_and_samples_dem() {
        let grid = MapGrid {
            lat0: -19.9,
            lon0: -43.9,
            tile_m: 250.0,
            cell_m: CELL_M,
        };
        let raster = flat_raster(&grid);
        let (roads, buildings) = parse_ways(OSM, &grid).unwrap();
        // Tile (0, 0): the road (x in [0, ~1046]) is cut to [0, 250] in x; the building
        // (x in [0, ~105], y in [0, ~56]) lies inside the tile and is kept whole.
        let (tile, nr, nb) = build_tile(&raster, &grid, &roads, &buildings, 0, 0).unwrap();
        assert_eq!((tile.x, tile.y), (0, 0));
        assert_eq!((tile.height.cols, tile.height.rows), (26, 26));
        assert!(tile.height.values.iter().all(|v| (*v - 100.0).abs() < 1e-3));
        assert_eq!((nr, nb), (1, 1));
        for p in &tile.roads[0].points {
            assert!(p[0] >= -1e-9 && p[0] <= 250.0 + 1e-9);
            assert!(p[1].abs() < 1e-6);
        }
        for p in &tile.buildings[0].points {
            assert!(p[0] >= -1e-9 && p[0] <= 250.0 + 1e-9);
            assert!(p[1] >= -1e-9 && p[1] <= 250.0 + 1e-9);
        }
        // Tile (5, 0) starts at x = 1250 m, east of the road end (~1046 m): nothing there.
        let (empty, nr, nb) = build_tile(&raster, &grid, &roads, &buildings, 5, 0).unwrap();
        assert!(empty.roads.is_empty() && empty.buildings.is_empty());
        assert_eq!((nr, nb), (0, 0));
    }
}
