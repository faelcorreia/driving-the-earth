//! Regional official height packages (F12). A package is a polygon (WGS84 lon/lat in the manifest)
//! plus an official surface on a 10 m grid of its CRS (SIRGAS 2000 UTM 18S-25S, i16 in decimeters)
//! and a feather mask. At bake time, inside the polygon and away from no-data gaps,
//! h = lerp(Copernicus, official, w); w ramps from 0 at the polygon edge to `feather_m` inside.
//! Overlapping regions: w = max, official height = weighted mean.
//!
//! Layout (both outside git):
//! - `cache/regions-src/<id>/`: raw source (contour shapefiles) and `region.json` (`RegionInfo`).
//!   `tilebake region <source-dir>` reads it.
//! - `cache/regions/<id>/`: the package (`manifest.json`, `height.i16`, `mask.u8`).
//! - `cache/regions/index.json`: `{"regions": [ids]}`, written by `tilebake region`; the bake loads
//!   exactly the packages listed there.
//! Heights are absolute official heights, not diffs.
use std::collections::VecDeque;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tileformat::HeightGrid;

use crate::dem::MapGrid;
use crate::shp;
use crate::utm;

/// Grid step of the official surface, in meters.
pub const GRID_M: f64 = 10.0;
/// Width of the ramp (mask) next to no-data nodes, in meters.
pub const RAMP_M: f64 = 100.0;
/// Radius of the contour points used to interpolate the surface at each node, in meters.
const SEARCH_M: f64 = 150.0;
/// Side of the buckets of the point index, in meters.
const BUCKET_M: f64 = 25.0;
/// Origin of the relative coordinates of the contour points (near Belo Horizonte, UTM 23S).
pub const ORIGIN_E: f64 = 610_000.0;
pub const ORIGIN_N: f64 = 7_800_000.0;
/// Mask value of a node fully inside the surface (stored as u8, 0..=250 in steps of 25).
const MASK_FULL: f64 = 250.0;
/// Default feather of the polygon edge, in meters.
pub const FEATHER_M: f64 = 500.0;
/// Unit of the stored heights (i16): decimeters.
const HEIGHT_UNIT_M: f64 = 0.1;

/// Name of the metadata file inside a source folder.
pub const INFO_FILE: &str = "region.json";
/// Name of the index file inside the packages folder.
pub const INDEX_FILE: &str = "index.json";

fn bad(msg: &str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, msg.to_string())
}

/// Source of the data: who published it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub provider: String,
    pub name: String,
}

/// Metadata of a region, as written in `region.json` next to the source shapefiles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionInfo {
    /// Package id: lowercase ASCII letters, digits and '-'. Also the folder name.
    pub id: String,
    pub source: Source,
    pub source_year: u32,
    pub attribution: String,
    /// Empty when unknown.
    #[serde(default)]
    pub license: String,
    /// EPSG of the grid, `EPSG:31978` to `EPSG:31985` (SIRGAS 2000 UTM 18S-25S).
    pub crs: String,
    /// Tie-break between overlapping regions (higher wins). Default 0.
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_feather")]
    pub feather_m: f64,
}

fn default_feather() -> f64 {
    FEATHER_M
}

/// Package manifest (`manifest.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub source: Source,
    pub source_year: u32,
    pub attribution: String,
    /// Empty when unknown.
    pub license: String,
    /// EPSG of the grid.
    pub crs: String,
    /// Absolute coordinates of node (0, 0) in the CRS, in meters.
    pub origin_e: f64,
    pub origin_n: f64,
    pub cell_m: f64,
    pub cols: usize,
    pub rows: usize,
    pub height_unit_m: f64,
    pub feather_m: f64,
    pub priority: i32,
    /// Region outer ring, `[lon, lat]` in WGS84 degrees.
    pub polygon_lonlat: Vec<[f64; 2]>,
}

/// One loaded regional package.
#[derive(Debug, Clone)]
pub struct Region {
    pub manifest: Manifest,
    /// Central meridian of the grid zone, in degrees.
    lon0: f64,
    /// Polygon in the CRS of the grid, derived from `polygon_lonlat`.
    poly_utm: Vec<[f64; 2]>,
    /// Official heights in units of `height_unit_m`, row-major (`row` = north).
    heights: Vec<i16>,
    /// Mask per node, 0..=250.
    mask: Vec<u8>,
}

/// Checks a package id: non-empty, lowercase ASCII letters, digits and '-' (it is a folder name).
pub fn validate_id(id: &str) -> io::Result<()> {
    let ok = !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(())
    } else {
        Err(bad(&format!("invalid region id '{id}': use lowercase letters, digits and '-'")))
    }
}

impl Region {
    /// Builds a region from its manifest and grid. Checks sizes and the CRS.
    pub fn new(manifest: Manifest, heights: Vec<i16>, mask: Vec<u8>) -> io::Result<Region> {
        let lon0 = utm::sirgas_zone_lon0(&manifest.crs).map_err(|e| bad(&e))?;
        if manifest.cols < 2 || manifest.rows < 2 || manifest.polygon_lonlat.len() < 3 {
            return Err(bad("manifest: grid or polygon too small"));
        }
        let n = manifest.cols * manifest.rows;
        if heights.len() != n || mask.len() != n {
            return Err(bad("package: grid sizes do not match the manifest"));
        }
        let poly_utm = manifest
            .polygon_lonlat
            .iter()
            .map(|p| {
                let (e, n) = utm::forward(p[1], p[0], lon0);
                [e, n]
            })
            .collect();
        Ok(Region {
            manifest,
            lon0,
            poly_utm,
            heights,
            mask,
        })
    }

    /// Reads the package in `dir`, validating the sizes of its files.
    pub fn load(dir: &Path) -> io::Result<Region> {
        let manifest: Manifest = serde_json::from_str(&fs::read_to_string(dir.join("manifest.json"))?)
            .map_err(|e| bad(&format!("manifest: {e}")))?;
        let n = manifest.cols.checked_mul(manifest.rows).ok_or_else(|| bad("manifest: grid too large"))?;
        let hb = fs::read(dir.join("height.i16"))?;
        let mask = fs::read(dir.join("mask.u8"))?;
        if hb.len() != 2 * n || mask.len() != n {
            return Err(bad("package: file sizes do not match the manifest"));
        }
        let heights = hb.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        Region::new(manifest, heights, mask)
    }

    /// Projects a WGS84 point to the CRS of this region's grid, `(easting, northing)` in meters.
    pub fn to_grid(&self, lat: f64, lon: f64) -> (f64, f64) {
        utm::forward(lat, lon, self.lon0)
    }

    /// Writes the package files to `dir` (creates it).
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let text = serde_json::to_string_pretty(&self.manifest).map_err(|e| bad(&e.to_string()))?;
        fs::write(dir.join("manifest.json"), text)?;
        let mut hb = Vec::with_capacity(self.heights.len() * 2);
        for h in &self.heights {
            hb.extend_from_slice(&h.to_le_bytes());
        }
        fs::write(dir.join("height.i16"), hb)?;
        fs::write(dir.join("mask.u8"), &self.mask)
    }

    /// `(weight, official height)` at UTM `(e, n)`, or `None` outside the grid.
    /// The weight is the polygon feather times the mask, so it is 0 at the edges of both.
    pub fn sample(&self, e: f64, n: f64) -> Option<(f64, f64)> {
        let m = &self.manifest;
        let fx = (e - m.origin_e) / m.cell_m;
        let fy = (n - m.origin_n) / m.cell_m;
        if !(fx >= 0.0 && fy >= 0.0 && fx <= (m.cols - 1) as f64 && fy <= (m.rows - 1) as f64) {
            return None;
        }
        let i0 = (fx.floor() as usize).min(m.cols - 2);
        let j0 = (fy.floor() as usize).min(m.rows - 2);
        let (tx, ty) = (fx - i0 as f64, fy - j0 as f64);
        let k = j0 * m.cols + i0;
        let corners = [k, k + 1, k + m.cols, k + m.cols + 1];
        let weights = [(1.0 - tx) * (1.0 - ty), tx * (1.0 - ty), (1.0 - tx) * ty, tx * ty];
        let (mut h, mut mask) = (0.0, 0.0);
        for (&idx, w) in corners.iter().zip(weights) {
            h += w * self.heights[idx] as f64;
            mask += w * self.mask[idx] as f64;
        }
        let mask = (mask / MASK_FULL).clamp(0.0, 1.0);
        let w = if mask > 0.0 {
            mask * poly_weight(&self.poly_utm, e, n, m.feather_m)
        } else {
            0.0
        };
        Some((w, h * m.height_unit_m))
    }
}

/// Weight of the polygon at `(x, y)`: 0 outside, smoothstep of the distance to the edge inside
/// (0 at the edge, 1 at `feather` meters inside).
pub fn poly_weight(poly: &[[f64; 2]], x: f64, y: f64, feather: f64) -> f64 {
    let n = poly.len();
    if n < 3 {
        return 0.0;
    }
    let mut inside = false;
    let mut d2min = f64::MAX;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[j], poly[i]);
        if (a[1] > y) != (b[1] > y) && x < a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]) {
            inside = !inside;
        }
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l2 = dx * dx + dy * dy;
        let t = if l2 > 0.0 {
            (((x - a[0]) * dx + (y - a[1]) * dy) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (px, py) = (a[0] + t * dx - x, a[1] + t * dy - y);
        d2min = d2min.min(px * px + py * py);
        j = i;
    }
    if !inside {
        return 0.0;
    }
    let t = (d2min.sqrt() / feather).min(1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Mixes the official heights into the Copernicus height `cop`. `parts` holds `(weight, official)`
/// for each region covering the point: the weight is the max of the weights, and the official height
/// is the weighted mean of the official heights, so overlapping regions do not double the effect.
pub fn blend(cop: f64, parts: impl IntoIterator<Item = (f64, f64)>) -> f64 {
    let (mut wsum, mut hsum, mut wmax) = (0.0f64, 0.0f64, 0.0f64);
    for (w, h) in parts {
        if w > 0.0 {
            wsum += w;
            hsum += w * h;
            wmax = wmax.max(w);
        }
    }
    if wsum <= 0.0 {
        cop
    } else {
        cop + wmax * (hsum / wsum - cop)
    }
}

/// Applies the regions to the height grid of a tile whose south-west corner is `(x0, y0)` in the
/// local plane of `grid`. Points are converted to UTM for the lookup.
pub fn apply_regions(height: &mut HeightGrid, grid: &MapGrid, regions: &[Region], x0: f64, y0: f64) {
    if regions.is_empty() {
        return;
    }
    let cols = height.cols as usize;
    for i in 0..cols {
        for j in 0..cols {
            let (lat, lon) = grid.latlon(x0 + j as f64 * grid.cell_m, y0 + i as f64 * grid.cell_m);
            let k = i * cols + j;
            let cop = height.values[k] as f64;
            let parts = regions.iter().filter_map(|r| {
                let (e, n) = r.to_grid(lat, lon);
                r.sample(e, n)
            });
            height.values[k] = blend(cop, parts) as f32;
        }
    }
}

/// Convex hull (monotone chain), counter-clockwise, without the repeated first point.
fn convex_hull(points: &mut [[f64; 2]]) -> Vec<[f64; 2]> {
    points.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut h: Vec<[f64; 2]> = Vec::new();
    for &q in points.iter() {
        while h.len() >= 2 && cross(h[h.len() - 2], h[h.len() - 1], q) <= 0.0 {
            h.pop();
        }
        h.push(q);
    }
    let lower = h.len() + 1;
    for &q in points.iter().rev().skip(1) {
        while h.len() >= lower && cross(h[h.len() - 2], h[h.len() - 1], q) <= 0.0 {
            h.pop();
        }
        h.push(q);
    }
    h.pop();
    h
}

/// Neighbours (4-connected) of node `k` in a grid of `cols` x `rows`.
fn neighbours(k: usize, cols: usize, rows: usize) -> impl Iterator<Item = usize> {
    let (i, j) = ((k % cols) as isize, (k / cols) as isize);
    [(-1, 0), (1, 0), (0, -1), (0, 1)].into_iter().filter_map(move |(di, dj)| {
        let (a, b) = (i + di, j + dj);
        (a >= 0 && b >= 0 && (a as usize) < cols && (b as usize) < rows).then(|| b as usize * cols + a as usize)
    })
}

/// A contour segment `a`-`b` (x, y relative to `ORIGIN_E`, `ORIGIN_N`) at elevation `z` in meters.
/// A contour point is a degenerate segment with `a == b`.
#[derive(Clone, Copy)]
pub struct Seg {
    pub a: [f32; 2],
    pub b: [f32; 2],
    pub z: f32,
}

/// Height at a node interpolated from the contour segments `cand` (`(squared distance, segment index,
/// x, y)`, where `(x, y)` is the closest point of the segment to the node, relative to the node).
/// The nearest segment gives the elevation `z1`. Then the nearest segments just below and just above
/// `z1` give the two contours that bracket the node, if they lie on opposite sides of it. The height is
/// linear in the distances to them (exact on a plane). Without such a bracket (above the highest or
/// below the lowest contour) it is `z1`.
fn contour_at(cand: &[(f64, u32, f64, f64)], segs: &[Seg]) -> f64 {
    let Some(&(_, i1, _, _)) = cand.iter().min_by(|a, b| a.0.total_cmp(&b.0)) else {
        return f64::NAN;
    };
    let z1 = segs[i1 as usize].z as f64;
    // Nearest segment of each side: (squared distance, x, y, z).
    let (mut lo, mut hi): (Option<(f64, f64, f64, f64)>, Option<(f64, f64, f64, f64)>) = (None, None);
    for &(d2, idx, vx, vy) in cand {
        let z = segs[idx as usize].z as f64;
        let side = if z < z1 { &mut lo } else if z > z1 { &mut hi } else { continue };
        if side.is_none_or(|(b, ..)| d2 < b) {
            *side = Some((d2, vx, vy, z));
        }
    }
    match (lo, hi) {
        (Some((dlo2, lx, ly, zlo)), Some((dhi2, hx, hy, zhi))) if lx * hx + ly * hy < 0.0 => {
            let (dlo, dhi) = (dlo2.sqrt(), dhi2.sqrt());
            zlo + (zhi - zlo) * dlo / (dlo + dhi)
        }
        _ => z1,
    }
}

/// Builds a package from contour points `[x, y, z]` (every point is a degenerate segment).
pub fn build(points: &[[f32; 3]], info: &RegionInfo) -> io::Result<Region> {
    let segs: Vec<Seg> = points.iter().map(|p| Seg { a: [p[0], p[1]], b: [p[0], p[1]], z: p[2] }).collect();
    build_segments(&segs, info)
}

/// Builds a package from contour segments (x, y relative to `ORIGIN_E`, `ORIGIN_N`, in the CRS of `info`;
/// z in meters). Each node takes the distance to the closest point of each segment, not only to vertices.
pub fn build_segments(segs: &[Seg], info: &RegionInfo) -> io::Result<Region> {
    validate_id(&info.id)?;
    let lon0 = utm::sirgas_zone_lon0(&info.crs).map_err(|e| bad(&e))?;
    if segs.is_empty() {
        return Err(bad("no contour points"));
    }
    let t0 = Instant::now();
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for s in segs {
        for p in [s.a, s.b] {
            x0 = x0.min(p[0] as f64);
            y0 = y0.min(p[1] as f64);
            x1 = x1.max(p[0] as f64);
            y1 = y1.max(p[1] as f64);
        }
    }
    let origin_e = ((x0 + ORIGIN_E - SEARCH_M) / GRID_M).floor() * GRID_M;
    let origin_n = ((y0 + ORIGIN_N - SEARCH_M) / GRID_M).floor() * GRID_M;
    let cols = ((x1 + ORIGIN_E + SEARCH_M - origin_e) / GRID_M).ceil() as usize + 1;
    let rows = ((y1 + ORIGIN_N + SEARCH_M - origin_n) / GRID_M).ceil() as usize + 1;
    let (rx0, ry0) = (origin_e - ORIGIN_E, origin_n - ORIGIN_N);

    // Bucket index (CSR) of the segments: each segment goes into every bucket its bounding box touches.
    let nbx = ((x1 - x0) / BUCKET_M) as usize + 1;
    let nby = ((y1 - y0) / BUCKET_M) as usize + 1;
    let bucket_range = |s: &Seg| {
        let (ax, bx) = (s.a[0].min(s.b[0]) as f64, s.a[0].max(s.b[0]) as f64);
        let (ay, by) = (s.a[1].min(s.b[1]) as f64, s.a[1].max(s.b[1]) as f64);
        let cx = |x: f64| (((x - x0) / BUCKET_M).max(0.0) as usize).min(nbx - 1);
        let cy = |y: f64| (((y - y0) / BUCKET_M).max(0.0) as usize).min(nby - 1);
        (cx(ax), cx(bx), cy(ay), cy(by))
    };
    let mut start = vec![0u32; nbx * nby + 1];
    for s in segs {
        let (x_lo, x_hi, y_lo, y_hi) = bucket_range(s);
        for by in y_lo..=y_hi {
            for bx in x_lo..=x_hi {
                start[by * nbx + bx + 1] += 1;
            }
        }
    }
    for b in 0..nbx * nby {
        start[b + 1] += start[b];
    }
    let mut fill = start.clone();
    let mut items = vec![0u32; start[nbx * nby] as usize];
    for (i, s) in segs.iter().enumerate() {
        let (x_lo, x_hi, y_lo, y_hi) = bucket_range(s);
        for by in y_lo..=y_hi {
            for bx in x_lo..=x_hi {
                let b = by * nbx + bx;
                items[fill[b] as usize] = i as u32;
                fill[b] += 1;
            }
        }
    }

    // Surface at every node; `valid` where at least 3 segments are within SEARCH_M.
    let n = rows * cols;
    let mut height = vec![0.0f32; n];
    let mut valid = vec![false; n];
    let rad = SEARCH_M;
    let r2 = SEARCH_M * SEARCH_M;
    let mut cand: Vec<(f64, u32, f64, f64)> = Vec::new();
    for j in 0..rows {
        for i in 0..cols {
            let (px, py) = (rx0 + i as f64 * GRID_M, ry0 + j as f64 * GRID_M);
            cand.clear();
            let bx_lo = (((px - rad - x0) / BUCKET_M).floor().max(0.0) as usize).min(nbx - 1);
            let bx_hi = (((px + rad - x0) / BUCKET_M).floor().max(0.0) as usize).min(nbx - 1);
            let by_lo = (((py - rad - y0) / BUCKET_M).floor().max(0.0) as usize).min(nby - 1);
            let by_hi = (((py + rad - y0) / BUCKET_M).floor().max(0.0) as usize).min(nby - 1);
            for by in by_lo..=by_hi {
                for bx in bx_lo..=bx_hi {
                    let b = by * nbx + bx;
                    for &idx in &items[start[b] as usize..start[b + 1] as usize] {
                        let s = segs[idx as usize];
                        let (ax, ay) = (s.a[0] as f64 - px, s.a[1] as f64 - py);
                        let (ux, uy) = (s.b[0] as f64 - s.a[0] as f64, s.b[1] as f64 - s.a[1] as f64);
                        let len2 = ux * ux + uy * uy;
                        let t = if len2 > 0.0 { (-(ax * ux + ay * uy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                        let (cx, cy) = (ax + t * ux, ay + t * uy);
                        let d2 = cx * cx + cy * cy;
                        if d2 <= r2 {
                            cand.push((d2, idx, cx, cy));
                        }
                    }
                }
            }
            if cand.len() < 3 {
                continue;
            }
            let k = j * cols + i;
            height[k] = contour_at(&cand, segs) as f32;
            valid[k] = true;
        }
    }
    if !valid.iter().any(|&v| v) {
        return Err(bad("no node has contour points around it"));
    }

    // Holes: fill with the nearest valid height. Mask: steps to the nearest hole, capped at the ramp.
    let ramp = (RAMP_M / GRID_M).round() as u8;
    let mut queue: VecDeque<usize> = (0..n).filter(|&k| valid[k]).collect();
    let mut seen = valid.clone();
    while let Some(k) = queue.pop_front() {
        for nb in neighbours(k, cols, rows) {
            if !seen[nb] {
                seen[nb] = true;
                height[nb] = height[k];
                queue.push_back(nb);
            }
        }
    }
    let mut steps = vec![u8::MAX; n];
    queue.clear();
    for k in 0..n {
        if !valid[k] {
            steps[k] = 0;
            queue.push_back(k);
        }
    }
    while let Some(k) = queue.pop_front() {
        let s = steps[k];
        if s >= ramp {
            continue;
        }
        for nb in neighbours(k, cols, rows) {
            if steps[nb] == u8::MAX {
                steps[nb] = s + 1;
                queue.push_back(nb);
            }
        }
    }
    let mask: Vec<u8> = steps
        .iter()
        .map(|&s| {
            let s = s.min(ramp) as u32;
            if s == ramp as u32 {
                MASK_FULL as u8
            } else {
                (s * (MASK_FULL as u32) / ramp as u32) as u8
            }
        })
        .collect();
    let heights: Vec<i16> = height
        .iter()
        .map(|&h| (h as f64 / HEIGHT_UNIT_M).round().clamp(i16::MIN as f64 + 1.0, i16::MAX as f64) as i16)
        .collect();

    // Polygon: convex hull of the points, absolute UTM coordinates.
    let mut abs: Vec<[f64; 2]> = segs
        .iter()
        .flat_map(|s| [s.a, s.b])
        .map(|p| [p[0] as f64 + ORIGIN_E, p[1] as f64 + ORIGIN_N])
        .collect();
    let polygon_utm = convex_hull(&mut abs);
    let polygon_lonlat = polygon_utm
        .iter()
        .map(|p| {
            let (lat, lon) = utm::inverse(p[0], p[1], lon0);
            [lon, lat]
        })
        .collect();
    eprintln!(
        "region {}: {} segments, grid {cols}x{rows} at {GRID_M} m, {} valid nodes, hull {} vertices, {:.1} s",
        info.id,
        segs.len(),
        valid.iter().filter(|&&v| v).count(),
        polygon_utm.len(),
        t0.elapsed().as_secs_f64()
    );
    Region::new(
        Manifest {
            id: info.id.clone(),
            source: info.source.clone(),
            source_year: info.source_year,
            attribution: info.attribution.clone(),
            license: info.license.clone(),
            crs: info.crs.clone(),
            origin_e,
            origin_n,
            cell_m: GRID_M,
            cols,
            rows,
            height_unit_m: HEIGHT_UNIT_M,
            feather_m: info.feather_m,
            priority: info.priority,
            polygon_lonlat,
        },
        heights,
        mask,
    )
}

/// Builds the package described by `source/region.json` from every `.shp` (with its `.dbf`, field
/// `COTA_CURVA`) in `source`. Writes it to `base/<id>/` and adds the id to `base/index.json`.
pub fn build_from_shapefiles(source: &Path, base: &Path) -> io::Result<Region> {
    let info: RegionInfo = serde_json::from_str(&fs::read_to_string(source.join(INFO_FILE))?)
        .map_err(|e| bad(&format!("{INFO_FILE}: {e}")))?;
    let mut shps: Vec<PathBuf> = fs::read_dir(source)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "shp"))
        .collect();
    shps.sort();
    let mut segs: Vec<Seg> = Vec::new();
    for shp_path in &shps {
        let t = Instant::now();
        let before = segs.len();
        let parts = shp::read_contours(shp_path, &shp_path.with_extension("dbf"), "COTA_CURVA", |ax, ay, bx, by, z| {
            let (ax, ay, bx, by) = (ax - ORIGIN_E, ay - ORIGIN_N, bx - ORIGIN_E, by - ORIGIN_N);
            let inside = [ax, ay, bx, by].iter().all(|v| v.abs() < 200_000.0);
            if z.is_finite() && inside {
                segs.push(Seg { a: [ax as f32, ay as f32], b: [bx as f32, by as f32], z: z as f32 });
            }
        })?;
        eprintln!(
            "{}: {parts} parts, {} segments kept, {:.1} s",
            shp_path.display(),
            segs.len() - before,
            t.elapsed().as_secs_f64()
        );
    }
    let region = build_segments(&segs, &info)?;
    region.save(&base.join(&info.id))?;
    add_to_index(base, &info.id)?;
    Ok(region)
}

/// Packages folder: `TILEBAKE_REGIONS` if set, else `cache_root/regions`.
pub fn regions_dir(cache_root: &Path) -> PathBuf {
    std::env::var_os("TILEBAKE_REGIONS")
        .map(PathBuf::from)
        .unwrap_or_else(|| cache_root.join("regions"))
}

/// Ids listed in `base/index.json` (empty when there is no index).
pub fn read_index(base: &Path) -> io::Result<Vec<String>> {
    let path = base.join(INDEX_FILE);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let index: Index = serde_json::from_str(&fs::read_to_string(&path)?).map_err(|e| bad(&format!("{INDEX_FILE}: {e}")))?;
    Ok(index.regions)
}

/// Adds `id` to `base/index.json` (creates it; keeps the ids sorted and unique).
pub fn add_to_index(base: &Path, id: &str) -> io::Result<()> {
    validate_id(id)?;
    fs::create_dir_all(base)?;
    let mut ids = read_index(base)?;
    if !ids.iter().any(|x| x == id) {
        ids.push(id.to_string());
    }
    ids.sort();
    let text = serde_json::to_string_pretty(&Index { regions: ids }).map_err(|e| bad(&e.to_string()))?;
    fs::write(base.join(INDEX_FILE), text + "\n")
}

/// Loads every package listed in `base/index.json`. Missing index: no regions.
pub fn load_indexed(base: &Path) -> io::Result<Vec<Region>> {
    read_index(base)?.iter().map(|id| Region::load(&base.join(id))).collect()
}

#[derive(Debug, Serialize, Deserialize)]
struct Index {
    regions: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utm::UTM23S_LON0;

    const CRS: &str = "EPSG:31983";

    /// Square `[0, size]^2` in local meters, placed at the absolute UTM origin of the package math.
    fn square(size: f64) -> Vec<[f64; 2]> {
        vec![
            [ORIGIN_E, ORIGIN_N],
            [ORIGIN_E + size, ORIGIN_N],
            [ORIGIN_E + size, ORIGIN_N + size],
            [ORIGIN_E, ORIGIN_N + size],
        ]
    }

    fn manifest(origin_e: f64, origin_n: f64, n: usize, feather_m: f64, polygon_utm: &[[f64; 2]]) -> Manifest {
        Manifest {
            id: "t".into(),
            source: Source {
                provider: "p".into(),
                name: "n".into(),
            },
            source_year: 2000,
            attribution: "t".into(),
            license: String::new(),
            crs: CRS.into(),
            origin_e,
            origin_n,
            cell_m: GRID_M,
            cols: n,
            rows: n,
            height_unit_m: HEIGHT_UNIT_M,
            feather_m,
            priority: 0,
            polygon_lonlat: polygon_utm
                .iter()
                .map(|p| {
                    let (lat, lon) = utm::inverse(p[0], p[1], UTM23S_LON0);
                    [lon, lat]
                })
                .collect(),
        }
    }

    /// Flat region: official height `h_m` everywhere on an `n` x `n` grid at absolute `(origin_e, origin_n)`.
    fn flat_region(origin_e: f64, origin_n: f64, n: usize, h_m: f64, polygon_utm: &[[f64; 2]]) -> Region {
        let m = manifest(origin_e, origin_n, n, FEATHER_M, polygon_utm);
        Region::new(m, vec![(h_m / HEIGHT_UNIT_M) as i16; n * n], vec![MASK_FULL as u8; n * n]).unwrap()
    }

    fn info(id: &str) -> RegionInfo {
        RegionInfo {
            id: id.into(),
            source: Source {
                provider: "p".into(),
                name: "n".into(),
            },
            source_year: 2015,
            attribution: "a".into(),
            license: String::new(),
            crs: CRS.into(),
            priority: 0,
            feather_m: FEATHER_M,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("region-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn blend_mixes_and_normalizes_weights() {
        assert!((blend(0.0, [(1.0, 10.0)]) - 10.0).abs() < 1e-12);
        assert!((blend(0.0, [(0.5, 10.0)]) - 5.0).abs() < 1e-12);
        // Mix with weight 0.25: 2 + 0.25 * (10 - 2) = 4.
        assert!((blend(2.0, [(0.25, 10.0)]) - 4.0).abs() < 1e-12);
        // No region covers the point: Copernicus stays.
        assert_eq!(blend(7.0, std::iter::empty()), 7.0);
        assert_eq!(blend(7.0, [(0.0, 99.0)]), 7.0);
    }

    #[test]
    fn overlap_uses_max_weight_and_normalized_height() {
        // Two equal weights: official height is the mean, weight is 0.5.
        let a = blend(0.0, [(0.5, 10.0), (0.5, 20.0)]);
        assert!((a - 7.5).abs() < 1e-12, "{a}");
        // Weights 1 and 0.3: w = max = 1, height = (10 * 1 + 20 * 0.3) / 1.3 = 16 / 1.3.
        let b = blend(0.0, [(1.0, 10.0), (0.3, 20.0)]);
        assert!((b - 16.0 / 1.3).abs() < 1e-12, "{b}");
        // Order does not matter.
        let c = blend(0.0, [(0.3, 20.0), (1.0, 10.0)]);
        assert!((b - c).abs() < 1e-12);
        // Two regions inside the feather of one another: (50 * 1 + 80 * 0.5) / 1.5 = 60.
        let d = blend(0.0, [(1.0, 50.0), (0.5, 80.0)]);
        assert!((d - 60.0).abs() < 1e-12, "{d}");
    }

    #[test]
    fn overlapping_regions_with_same_height_do_not_double_it() {
        let r1 = flat_region(ORIGIN_E, ORIGIN_N, 120, 50.0, &square(1000.0));
        let r2 = flat_region(ORIGIN_E, ORIGIN_N, 120, 50.0, &square(1000.0));
        let parts = [&r1, &r2].map(|r| r.sample(ORIGIN_E + 500.0, ORIGIN_N + 500.0).unwrap());
        // Deep inside both: weight 1 each, height 50. Blend with cop 0 gives 50, not 100.
        let h = blend(0.0, parts.iter().copied());
        assert!((h - 50.0).abs() < 1e-9, "{h}");
    }

    #[test]
    fn border_has_no_step() {
        // Official 50 m, Copernicus 0 m: the blend must go 0 -> 50 smoothly across the feather.
        let polygon = square(2000.0);
        let r = flat_region(ORIGIN_E - 100.0, ORIGIN_N - 100.0, 260, 50.0, &polygon);
        let mut prev: Option<f64> = None;
        let mut max_step = 0.0f64;
        let mut x = -300.0;
        while x <= 2300.0 {
            let (w, h) = r.sample(ORIGIN_E + x, ORIGIN_N + 1000.0).unwrap_or((0.0, 0.0));
            let v = blend(0.0, [(w, h)]);
            if let Some(p) = prev {
                max_step = max_step.max((v - p).abs());
            }
            prev = Some(v);
            x += 1.0;
        }
        // Smoothstep over 500 m: slope at most 1.5 * 50 / 500 = 0.15 m per m.
        assert!(max_step <= 0.16, "step {max_step}");
        // Outside the polygon nothing is changed; well inside it is fully official.
        assert!(blend(0.0, [r.sample(ORIGIN_E - 50.0, ORIGIN_N + 1000.0).unwrap()]).abs() < 1e-9);
        assert!((blend(0.0, [r.sample(ORIGIN_E + 1000.0, ORIGIN_N + 1000.0).unwrap()]) - 50.0).abs() < 1e-9);
    }

    #[test]
    fn poly_weight_edge_and_inside() {
        let p = [[0.0, 0.0], [2000.0, 0.0], [2000.0, 2000.0], [0.0, 2000.0]];
        assert_eq!(poly_weight(&p, -1.0, 1000.0, FEATHER_M), 0.0);
        assert!(poly_weight(&p, 0.0, 1000.0, FEATHER_M).abs() < 1e-12);
        assert!((poly_weight(&p, 1000.0, 1000.0, FEATHER_M) - 1.0).abs() < 1e-12);
        let mid = poly_weight(&p, 250.0, 1000.0, FEATHER_M);
        assert!(mid > 0.0 && mid < 1.0);
    }

    #[test]
    fn surface_fits_a_plane_from_contour_points() {
        // Points on a 2 m grid over a 400 m square, heights of the plane z = 0.2 x + 0.1 y + 500.
        let mut pts = Vec::new();
        for i in 0..200 {
            for j in 0..200 {
                let (x, y) = (i as f64 * 2.0, j as f64 * 2.0);
                pts.push([x as f32, y as f32, (0.2 * x + 0.1 * y + 500.0) as f32]);
            }
        }
        let r = build(&pts, &info("plane")).unwrap();
        for (x, y) in [(100.0, 100.0), (200.0, 150.0), (300.0, 300.0)] {
            let (_, h) = r.sample(ORIGIN_E + x, ORIGIN_N + y).unwrap();
            let expected = 0.2 * x + 0.1 * y + 500.0;
            assert!((h - expected).abs() < 0.06, "({x}, {y}): {h} vs {expected}");
        }
    }

    #[test]
    fn surface_from_contour_lines_only_reconstructs_a_ramp() {
        // Plane z = 0.2 x + 0.1 y + 500, sampled only on its contour lines (1 m interval, points every ~1 m).
        let mut pts = Vec::new();
        for c in 480..=640 {
            for i in 0..=1000 {
                let x = -100.0 + i as f64 * 0.6;
                let y = 10.0 * (c - 500) as f64 - 2.0 * x;
                if (-100.0..=500.0).contains(&y) {
                    pts.push([x as f32, y as f32, c as f32]);
                }
            }
        }
        let r = build(&pts, &info("ramp")).unwrap();
        for (x, y) in [(100.0, 100.0), (200.0, 150.0), (300.0, 300.0), (150.0, 250.0), (120.0, 330.0)] {
            let (_, h) = r.sample(ORIGIN_E + x, ORIGIN_N + y).unwrap();
            let expected = 0.2 * x + 0.1 * y + 500.0;
            assert!((h - expected).abs() < 0.05, "({x}, {y}): {h} vs {expected}");
        }
    }

    #[test]
    fn surface_from_contour_lines_only_reconstructs_a_cone() {
        // Cone z = 100 - distance to (203, 197), sampled only on its circular contours (1 m interval, points every ~1 m).
        let (cx, cy) = (203.0, 197.0);
        let mut pts = Vec::new();
        for c in 1..=99 {
            let radius = (100 - c) as f64;
            let n = (std::f64::consts::TAU * radius).ceil() as usize;
            for i in 0..n {
                let a = std::f64::consts::TAU * i as f64 / n as f64;
                pts.push([(cx + radius * a.cos()) as f32, (cy + radius * a.sin()) as f32, c as f32]);
            }
        }
        let r = build(&pts, &info("cone")).unwrap();
        for (x, y) in [(230.0, 200.0), (200.0, 150.0), (240.0, 230.0), (290.0, 200.0), (250.0, 260.0), (120.0, 200.0)] {
            let (_, h) = r.sample(ORIGIN_E + x, ORIGIN_N + y).unwrap();
            let expected = 100.0 - ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            assert!((h - expected).abs() < 0.1, "({x}, {y}): {h} vs {expected}");
        }
    }

    #[test]
    fn surface_from_contour_segments_reconstructs_a_ramp() {
        // Plane z = 0.2 x + 0.1 y + 500; each contour line is one long segment (clipped to the square [-100, 500]^2).
        let mut segs = Vec::new();
        for c in 480..=640 {
            let y_of = |x: f64| 10.0 * (c - 500) as f64 - 2.0 * x;
            let x_lo = (-100.0f64).max((10.0 * (c - 500) as f64 - 500.0) / 2.0);
            let x_hi = 500.0f64.min((10.0 * (c - 500) as f64 + 100.0) / 2.0);
            if x_lo < x_hi {
                let (a, b) = ([x_lo as f32, y_of(x_lo) as f32], [x_hi as f32, y_of(x_hi) as f32]);
                segs.push(Seg { a, b, z: c as f32 });
            }
        }
        let r = build_segments(&segs, &info("ramp-segments")).unwrap();
        for (x, y) in [(250.0, 200.0), (300.0, 150.0), (230.0, 260.0)] {
            let (_, h) = r.sample(ORIGIN_E + x, ORIGIN_N + y).unwrap();
            let expected = 0.2 * x + 0.1 * y + 500.0;
            assert!((h - expected).abs() < 0.05, "({x}, {y}): {h} vs {expected}");
        }
    }

    #[test]
    fn package_round_trip_keeps_samples_and_polygon() {
        let r = flat_region(ORIGIN_E - 100.0, ORIGIN_N - 100.0, 40, 12.3, &square(200.0));
        let dir = temp_dir("roundtrip");
        r.save(&dir).unwrap();
        let back = Region::load(&dir).unwrap();
        assert_eq!(back.heights, r.heights);
        assert_eq!(back.mask, r.mask);
        assert_eq!(back.manifest.polygon_lonlat, r.manifest.polygon_lonlat);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn polygon_is_stored_lon_lat_and_converts_back_to_the_grid() {
        let utm_poly = square(2000.0);
        let r = flat_region(ORIGIN_E, ORIGIN_N, 10, 1.0, &utm_poly);
        // Stored as [lon, lat] WGS84 near Belo Horizonte (lat ~ -19.9, lon ~ -44).
        for [lon, lat] in &r.manifest.polygon_lonlat {
            assert!((-45.0..-43.0).contains(lon) && (-21.0..-19.0).contains(lat), "{lon} {lat}");
        }
        // Converted back on load: within a millimeter of the original UTM vertices.
        for (a, b) in r.poly_utm.iter().zip(&utm_poly) {
            assert!((a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn index_lists_sorted_unique_ids_and_loads_them() {
        let base = temp_dir("index");
        assert!(read_index(&base).unwrap().is_empty());
        assert!(load_indexed(&base).unwrap().is_empty());
        flat_region(ORIGIN_E, ORIGIN_N, 10, 1.0, &square(200.0)).save(&base.join("b")).unwrap();
        flat_region(ORIGIN_E, ORIGIN_N, 10, 2.0, &square(200.0)).save(&base.join("a")).unwrap();
        add_to_index(&base, "b").unwrap();
        add_to_index(&base, "a").unwrap();
        add_to_index(&base, "a").unwrap();
        assert_eq!(read_index(&base).unwrap(), vec!["a", "b"]);
        assert_eq!(load_indexed(&base).unwrap().len(), 2);
        // Only the listed packages are loaded: a folder not in the index is ignored.
        flat_region(ORIGIN_E, ORIGIN_N, 10, 3.0, &square(200.0)).save(&base.join("c")).unwrap();
        assert_eq!(load_indexed(&base).unwrap().len(), 2);
        // An index that points to a missing package is an error.
        add_to_index(&base, "ghost").unwrap();
        assert!(load_indexed(&base).is_err());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn region_info_defaults_and_validation() {
        let text = r#"{"id":"x","source":{"provider":"p","name":"n"},"source_year":2015,"attribution":"a","crs":"EPSG:31983"}"#;
        let i: RegionInfo = serde_json::from_str(text).unwrap();
        assert_eq!((i.license.as_str(), i.priority, i.feather_m), ("", 0, FEATHER_M));
        assert!(validate_id("br-mg-belo-horizonte").is_ok());
        assert!(validate_id("../x").is_err() && validate_id("Upper").is_err() && validate_id("").is_err());
        let pts = [[0.0f32, 0.0, 1.0], [10.0, 0.0, 1.0], [0.0, 10.0, 1.0]];
        let mut bad_crs = info("x");
        bad_crs.crs = "EPSG:4326".into();
        assert!(build(&pts, &bad_crs).is_err());
        assert!(build(&pts, &info("../escape")).is_err());
    }
}
