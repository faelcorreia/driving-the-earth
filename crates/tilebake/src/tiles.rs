//! Raw data (OSM + DEM already in cache) -> tiles in `cache/<id>/tiles/`.
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;
use tileformat::{HeightGrid, MapMeta, Polygon, Polyline, Tile};

use crate::clip::{clip_polygon, clip_polyline, Rect};
use crate::dem::{download_dem, read_smoothed_dem, tile_heights_any, MapGrid, Raster};
use crate::osm;
use crate::profile;
use crate::region::{apply_regions, Region};

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
    /// Size of `cache/<id>/tiles/` after the bake, in bytes.
    pub tiles_bytes: u64,
    pub elapsed: Duration,
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

/// Geographic bounding box `(south, west, north, east)` of all the covering tiles.
/// The OSM query uses it, so every way that touches a baked tile is downloaded.
pub fn covering_bbox(meta: &MapMeta) -> (f64, f64, f64, f64) {
    let grid = MapGrid::from_meta(meta);
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in covering_tiles(meta.radius_m, meta.tile_size_m) {
        let r = tile_rect(x, y, meta.tile_size_m);
        x0 = x0.min(r.x0);
        y0 = y0.min(r.y0);
        x1 = x1.max(r.x1);
        y1 = y1.max(r.y1);
    }
    let (south, west) = grid.latlon(x0, y0);
    let (north, east) = grid.latlon(x1, y1);
    (south, west, north, east)
}

/// Adds one way (Overpass element with `out body geom`) to `roads` or `buildings`, in the local plane.
pub fn add_way(e: &Value, grid: &MapGrid, roads: &mut Vec<Polyline>, buildings: &mut Vec<Polygon>) {
    let Some(geom) = e["geometry"].as_array() else {
        return;
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
                z: Vec::new(),
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

/// Roads and buildings from a raw Overpass response (`out body geom`), in the local plane.
pub fn parse_ways(overpass_json: &str, grid: &MapGrid) -> io::Result<(Vec<Polyline>, Vec<Polygon>)> {
    let v: Value = serde_json::from_str(overpass_json).map_err(invalid_data)?;
    let elements = v["elements"]
        .as_array()
        .ok_or_else(|| invalid_data("Overpass response has no 'elements'"))?;
    let mut roads = Vec::new();
    let mut buildings = Vec::new();
    for e in elements {
        add_way(e, grid, &mut roads, &mut buildings);
    }
    Ok((roads, buildings))
}

/// Builds tile `(x, y)`: DEM grid plus the parts of the roads and buildings inside it.
/// Returns the tile and the number of road and building pieces.
pub fn build_tile(
    rasters: &[Raster],
    grid: &MapGrid,
    roads: &[Polyline],
    buildings: &[Polygon],
    regions: &[Region],
    x: i32,
    y: i32,
) -> io::Result<(Tile, usize, usize)> {
    let rect = tile_rect(x, y, grid.tile_m);
    let mut height = tile_heights_any(rasters, grid, x, y)?;
    apply_regions(&mut height, grid, regions, rect.x0, rect.y0);
    Ok(build_tile_from(height, grid, roads, buildings, x, y))
}

/// Tile `(x, y)` from its final height grid (regions already applied).
pub fn build_tile_from(
    height: HeightGrid,
    grid: &MapGrid,
    roads: &[Polyline],
    buildings: &[Polygon],
    x: i32,
    y: i32,
) -> (Tile, usize, usize) {
    let rect = tile_rect(x, y, grid.tile_m);
    let mut tile_roads = Vec::new();
    for road in roads {
        for points in clip_polyline(&road.points, rect) {
            let z = if road.z.is_empty() {
                Vec::new()
            } else {
                profile::z_for_piece(&road.points, &road.z, &points)
            };
            tile_roads.push(Polyline {
                highway: road.highway.clone(),
                points,
                z,
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
    (tile, counts.0, counts.1)
}

/// Regional packages listed in `index.json` of the packages folder (see `region::regions_dir`).
pub fn load_regions(cache_root: &Path) -> io::Result<Vec<Region>> {
    crate::region::load_indexed(&crate::region::regions_dir(cache_root))
}

/// Index from tile `(x, y)` to the positions of the items whose bounding box touches that tile.
/// `points_of` gives the vertices of an item (a road or the ring of a building).
pub fn index_by_tile<T>(
    items: &[T],
    points_of: impl Fn(&T) -> &[[f64; 2]],
    tile_m: f64,
) -> std::collections::HashMap<(i32, i32), Vec<usize>> {
    let mut index: std::collections::HashMap<(i32, i32), Vec<usize>> = std::collections::HashMap::new();
    for (i, item) in items.iter().enumerate() {
        let pts = points_of(item);
        if pts.is_empty() {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in pts {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
        // One tile of margin on each side: an item that only touches a tile border still
        // clips to a (degenerate) piece there, as in a full scan, so that tile must see it.
        let (tx0, ty0) = ((x0 / tile_m).floor() as i32 - 1, (y0 / tile_m).floor() as i32 - 1);
        let (tx1, ty1) = ((x1 / tile_m).floor() as i32 + 1, (y1 / tile_m).floor() as i32 + 1);
        for tx in tx0..=tx1 {
            for ty in ty0..=ty1 {
                index.entry((tx, ty)).or_default().push(i);
            }
        }
    }
    index
}

/// Bakes all the tiles covering the radius from the raw data in `cache_root/<id>/raw/osm/`
/// (blocks, deduplicated by way id), writes them to `cache_root/<id>/tiles/`, and then sets
/// `meta.complete = true`. Needs `meta.json` and the blocks; downloads the DEM tile only if it is missing.
/// Final terrain height at a point: bilinear in the grid of the tile that contains it.
fn terrain_at(heights: &HashMap<(i32, i32), HeightGrid>, grid: &MapGrid, p: [f64; 2]) -> Option<f64> {
    let tile_m = grid.tile_m;
    let (tx, ty) = ((p[0] / tile_m).floor() as i32, (p[1] / tile_m).floor() as i32);
    let h = heights.get(&(tx, ty))?;
    let n = h.cols as usize;
    let cell = tile_m / (n - 1) as f64;
    let fx = ((p[0] - tx as f64 * tile_m) / cell).clamp(0.0, (n - 1) as f64);
    let fy = ((p[1] - ty as f64 * tile_m) / cell).clamp(0.0, (n - 1) as f64);
    let (j, i) = ((fx.floor() as usize).min(n - 2), (fy.floor() as usize).min(n - 2));
    let (u, v) = (fx - j as f64, fy - i as f64);
    let at = |r: usize, c: usize| h.values[r * n + c] as f64;
    Some(
        at(i, j) * (1.0 - u) * (1.0 - v)
            + at(i, j + 1) * u * (1.0 - v)
            + at(i + 1, j) * (1.0 - u) * v
            + at(i + 1, j + 1) * u * v,
    )
}

pub fn bake_tiles(cache_root: &Path, id: &str) -> io::Result<BakeSummary> {
    let started = Instant::now();
    let dir = cache_root.join(id);
    let meta_path = dir.join("meta.json");
    let mut meta: MapMeta = serde_json::from_str(&fs::read_to_string(&meta_path)?).map_err(invalid_data)?;
    let grid = MapGrid::from_meta(&meta);

    let mut roads = Vec::new();
    let mut buildings = Vec::new();
    osm::visit_ways(&dir.join("raw").join("osm"), &meta, |e| {
        add_way(e, &grid, &mut roads, &mut buildings)
    })?;
    // One Copernicus tile per 1 degree cell that the baked area touches; missing ones are downloaded.
    let (south, west, north, east) = covering_bbox(&meta);
    let mut rasters = Vec::new();
    for la in south.floor() as i32..=north.floor() as i32 {
        for lo in west.floor() as i32..=east.floor() as i32 {
            let (tif, _) = download_dem(cache_root, id, la as f64 + 0.5, lo as f64 + 0.5)?;
            rasters.push(read_smoothed_dem(&tif)?);
        }
    }
    let regions = load_regions(cache_root)?;

    // Final terrain of every tile (regions applied), shared by the road profiles and the tiles.
    let mut heights: HashMap<(i32, i32), HeightGrid> = HashMap::new();
    for (x, y) in covering_tiles(meta.radius_m, meta.tile_size_m) {
        let rect = tile_rect(x, y, grid.tile_m);
        let mut h = tile_heights_any(&rasters, &grid, x, y)?;
        apply_regions(&mut h, &grid, &regions, rect.x0, rect.y0);
        heights.insert((x, y), h);
    }
    // Road profiles: each way whole (not cut by tile), densified and smoothed over the final terrain.
    let ways: Vec<Vec<[f64; 2]>> = roads.iter().map(|r| r.points.clone()).collect();
    let pins = profile::pinned_nodes(&ways);
    let terrain = |p: [f64; 2]| terrain_at(&heights, &grid, p);
    for road in roads.iter_mut() {
        if let Some(z) = profile::way_profile(&road.points, &terrain, &|p| pins.contains(&profile::node_key(p))) {
            road.points = profile::densify(&road.points);
            road.z = z;
        }
    }
    let road_index = index_by_tile(&roads, |r| &r.points, meta.tile_size_m);
    let building_index = index_by_tile(&buildings, |b| &b.points, meta.tile_size_m);

    let mut summary = BakeSummary {
        tiles: 0,
        road_pieces: 0,
        building_pieces: 0,
        tiles_bytes: 0,
        elapsed: Duration::ZERO,
    };
    for (x, y) in covering_tiles(meta.radius_m, meta.tile_size_m) {
        // Only the ways whose bounding box touches the tile: the others clip to nothing.
        let tile_roads: Vec<Polyline> = road_index
            .get(&(x, y))
            .map(|ids| ids.iter().map(|&i| roads[i].clone()).collect())
            .unwrap_or_default();
        let tile_buildings: Vec<Polygon> = building_index
            .get(&(x, y))
            .map(|ids| ids.iter().map(|&i| buildings[i].clone()).collect())
            .unwrap_or_default();
        let final_heights = heights.remove(&(x, y)).ok_or_else(|| invalid_data("tile without terrain"))?;
        let (tile, r, b) = build_tile_from(final_heights, &grid, &tile_roads, &tile_buildings, x, y);
        tileformat::write_tile(cache_root, id, &tile)?;
        summary.tiles += 1;
        summary.road_pieces += r;
        summary.building_pieces += b;
    }

    meta.complete = true;
    fs::write(&meta_path, serde_json::to_string_pretty(&meta).map_err(invalid_data)?)?;
    summary.tiles_bytes = crate::dir_bytes(&dir.join("tiles"))?;
    summary.elapsed = started.elapsed();
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
    fn covering_bbox_spans_the_covering_tiles() {
        let meta = MapMeta {
            id: "t".into(),
            name: "t".into(),
            center_lat: -19.9,
            center_lon: -43.9,
            radius_m: 1000.0,
            tile_size_m: 250.0,
            created_unix: 0,
            complete: false,
        };
        let (s, w, n, e) = covering_bbox(&meta);
        assert!(s < meta.center_lat && meta.center_lat < n);
        assert!(w < meta.center_lon && meta.center_lon < e);
        // Covering tiles run from x = -1000 m to 1000 m and from y = -1000 m to 1000 m.
        let grid = MapGrid::from_meta(&meta);
        let (lat_top, _) = grid.latlon(0.0, 1000.0);
        let (lat_bottom, _) = grid.latlon(0.0, -1000.0);
        assert!((n - lat_top).abs() < 1e-12 && (s - lat_bottom).abs() < 1e-12, "{s} {n}");
        let (_, lon_left) = grid.latlon(-1000.0, 0.0);
        let (_, lon_right) = grid.latlon(1000.0, 0.0);
        assert!((w - lon_left).abs() < 1e-12 && (e - lon_right).abs() < 1e-12);
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
    fn index_gives_the_same_tiles_as_a_full_scan() {
        let grid = MapGrid {
            lat0: -19.9,
            lon0: -43.9,
            tile_m: 250.0,
            cell_m: CELL_M,
        };
        let raster = flat_raster(&grid);
        let (roads, buildings) = parse_ways(OSM, &grid).unwrap();
        let road_index = index_by_tile(&roads, |r| &r.points, 250.0);
        let building_index = index_by_tile(&buildings, |b| &b.points, 250.0);
        for (x, y) in covering_tiles(1000.0, 250.0) {
            let sub_roads: Vec<Polyline> = road_index
                .get(&(x, y))
                .map(|ids| ids.iter().map(|&i| roads[i].clone()).collect())
                .unwrap_or_default();
            let sub_buildings: Vec<Polygon> = building_index
                .get(&(x, y))
                .map(|ids| ids.iter().map(|&i| buildings[i].clone()).collect())
                .unwrap_or_default();
            let (full, _, _) = build_tile(std::slice::from_ref(&raster), &grid, &roads, &buildings, &[], x, y).unwrap();
            let (part, _, _) = build_tile(std::slice::from_ref(&raster), &grid, &sub_roads, &sub_buildings, &[], x, y).unwrap();
            let road_pts = |t: &Tile| t.roads.iter().map(|r| (r.highway.clone(), r.points.clone())).collect::<Vec<_>>();
            let bld_pts = |t: &Tile| t.buildings.iter().map(|b| b.points.clone()).collect::<Vec<_>>();
            assert_eq!(road_pts(&full), road_pts(&part), "roads of tile ({x},{y})");
            assert_eq!(bld_pts(&full), bld_pts(&part), "buildings of tile ({x},{y})");
        }
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
        let (tile, nr, nb) = build_tile(std::slice::from_ref(&raster), &grid, &roads, &buildings, &[], 0, 0).unwrap();
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
        let (empty, nr, nb) = build_tile(std::slice::from_ref(&raster), &grid, &roads, &buildings, &[], 5, 0).unwrap();
        assert!(empty.roads.is_empty() && empty.buildings.is_empty());
        assert_eq!((nr, nb), (0, 0));
    }
}
