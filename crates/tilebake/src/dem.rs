//! Copernicus GLO-30 DEM (DSM) -> height grid of each tile.
//!
//! Tile grid: local plane in meters, origin at the center of `meta.json`
//! (x = east, y = north). Tile `(x, y)` covers `[x*tile, (x+1)*tile]` x
//! `[y*tile, (y+1)*tile]`. The grid has samples at the corners, with step `CELL_M`:
//! `cols = rows = tile/CELL_M + 1`, `values[row * cols + col]` with `row` to the north
//! and `col` to the east. Neighboring tiles share their edge.
use std::fmt::Display;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;
use tileformat::{HeightGrid, MapMeta};

const USER_AGENT: &str = "driving-the-earth/0.1 (tilebake; projeto pessoal)";
const COPERNICUS_BASE: &str = "https://copernicus-dem-30m.s3.amazonaws.com";
/// Step of the tile grid, in meters.
pub const CELL_M: f64 = 10.0;
/// Mean Earth radius, for the local plane.
const EARTH_RADIUS_M: f64 = 6_371_000.0;
/// Values below this (and non-finite ones) mean "no data".
const NODATA_LIMIT: f32 = -9000.0;
/// GeoTIFF tags: ModelPixelScale and ModelTiepoint.
const TAG_PIXEL_SCALE: u16 = 33550;
const TAG_TIEPOINT: u16 = 33922;

/// Terrain read from the GeoTIFF, already smoothed. Pixel (c, r) is centered at
/// `lon0 + (c + 0.5) * step_lon` and `lat0 - (r + 0.5) * step_lat`.
#[derive(Debug, Clone)]
pub struct Raster {
    pub width: usize,
    pub height: usize,
    /// Upper-left corner of the raster, in degrees.
    pub lon0: f64,
    pub lat0: f64,
    /// Pixel size in degrees (positive).
    pub step_lon: f64,
    pub step_lat: f64,
    /// Heights, row-major; `NaN` where there is no data.
    pub values: Vec<f32>,
}

/// Map grid: converts tile coordinates to lat/lon and defines the tile.
#[derive(Debug, Clone, Copy)]
pub struct MapGrid {
    pub lat0: f64,
    pub lon0: f64,
    pub tile_m: f64,
    pub cell_m: f64,
}

impl MapGrid {
    pub fn from_meta(meta: &MapMeta) -> Self {
        MapGrid {
            lat0: meta.center_lat,
            lon0: meta.center_lon,
            tile_m: meta.tile_size_m,
            cell_m: CELL_M,
        }
    }

    /// Meters per degree of latitude and longitude, at the center of the map.
    fn meters_per_degree(&self) -> (f64, f64) {
        let m_lat = EARTH_RADIUS_M.to_radians();
        (m_lat, m_lat * self.lat0.to_radians().cos())
    }

    /// (lat, lon) of the point `(x_m, y_m)` on the local plane (east, north).
    pub fn latlon(&self, x_m: f64, y_m: f64) -> (f64, f64) {
        let (m_lat, m_lon) = self.meters_per_degree();
        (self.lat0 + y_m / m_lat, self.lon0 + x_m / m_lon)
    }

    /// `(x_m, y_m)` on the local plane (east, north) of `(lat, lon)`. Inverse of `latlon`.
    pub fn local(&self, lat: f64, lon: f64) -> (f64, f64) {
        let (m_lat, m_lon) = self.meters_per_degree();
        ((lon - self.lon0) * m_lon, (lat - self.lat0) * m_lat)
    }
}

fn other_err<E: Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}

fn invalid_data<E: Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

/// Name of the Copernicus tile that covers `(lat, lon)`, by its SW corner (e.g. S20_00_W044_00).
pub fn tile_name(lat: f64, lon: f64) -> String {
    let la = lat.floor() as i32;
    let lo = lon.floor() as i32;
    let ns = if la >= 0 { 'N' } else { 'S' };
    let ew = if lo >= 0 { 'E' } else { 'W' };
    format!(
        "Copernicus_DSM_COG_10_{ns}{:02}_00_{ew}{:03}_00_DEM",
        la.abs(),
        lo.abs()
    )
}

/// Downloads the tile that covers `(lat, lon)` to `cache_root/<id>/raw/copernicus/`.
/// Returns (path, came_from_cache). If it already exists, it is not downloaded again.
pub fn download_dem(
    cache_root: &Path,
    id: &str,
    lat: f64,
    lon: f64,
) -> io::Result<(PathBuf, bool)> {
    let name = tile_name(lat, lon);
    let dir = cache_root.join(id).join("raw").join("copernicus");
    fs::create_dir_all(&dir)?;
    let dest = dir.join(format!("{name}.tif"));
    if dest.exists() {
        return Ok((dest, true));
    }
    let url = format!("{COPERNICUS_BASE}/{name}/{name}.tif");
    let resp = ureq::get(&url)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(other_err)?;
    let part = dir.join(format!("{name}.tif.part"));
    {
        let mut body = resp.into_reader();
        let mut file = fs::File::create(&part)?;
        io::copy(&mut body, &mut file)?;
        file.sync_all()?;
    }
    fs::rename(&part, &dest)?;
    Ok((dest, false))
}

/// Reads the GeoTIFF (first IFD, float32) and applies Gaussian smoothing (sigma 1 px,
/// radius 2 px), to remove canopy and roof spikes from the DSM.
pub fn read_smoothed_dem(path: &Path) -> io::Result<Raster> {
    let file = fs::File::open(path)?;
    let mut dec = Decoder::new(io::BufReader::new(file)).map_err(invalid_data)?;
    let (w, h) = dec.dimensions().map_err(invalid_data)?;
    let scale = dec
        .get_tag_f64_vec(Tag::Unknown(TAG_PIXEL_SCALE))
        .map_err(invalid_data)?;
    let tiepoint = dec
        .get_tag_f64_vec(Tag::Unknown(TAG_TIEPOINT))
        .map_err(invalid_data)?;
    if scale.len() < 2 || tiepoint.len() < 6 || tiepoint[0] != 0.0 || tiepoint[1] != 0.0 {
        return Err(invalid_data(
            "GeoTIFF without simple georeferencing (ModelPixelScale/Tiepoint)",
        ));
    }
    let raw = match dec.read_image().map_err(invalid_data)? {
        DecodingResult::F32(v) => v,
        _ => return Err(invalid_data("expected float32 DEM")),
    };
    let (w, h) = (w as usize, h as usize);
    if raw.len() != w * h {
        return Err(invalid_data("DEM size does not match dimensions"));
    }
    let clean: Vec<f32> = raw
        .into_iter()
        .map(|v| {
            if v.is_finite() && v > NODATA_LIMIT {
                v
            } else {
                f32::NAN
            }
        })
        .collect();
    let values = smooth(&clean, w, h);
    Ok(Raster {
        width: w,
        height: h,
        lon0: tiepoint[3],
        lat0: tiepoint[4],
        step_lon: scale[0],
        step_lat: scale[1],
        values,
    })
}

/// Separable 5-tap Gaussian (sigma 1), ignoring NaN and normalizing by the valid weights.
fn smooth(src: &[f32], w: usize, h: usize) -> Vec<f32> {
    let sigma: f32 = 1.0;
    let kernel: [f32; 5] = {
        let mut k = [0.0f32; 5];
        for (i, v) in k.iter_mut().enumerate() {
            let d = i as f32 - 2.0;
            *v = (-(d * d) / (2.0 * sigma * sigma)).exp();
        }
        let sum: f32 = k.iter().sum();
        k.map(|v| v / sum)
    };
    let horizontal = pass_1d(src, w, h, 1, 0, &kernel);
    pass_1d(&horizontal, w, h, 0, 1, &kernel)
}

fn pass_1d(src: &[f32], w: usize, h: usize, dx: isize, dy: isize, k: &[f32; 5]) -> Vec<f32> {
    let mut out = vec![f32::NAN; w * h];
    for r in 0..h {
        for c in 0..w {
            if src[r * w + c].is_nan() {
                continue;
            }
            let (mut acc, mut weight) = (0.0f32, 0.0f32);
            for (i, &kw) in k.iter().enumerate() {
                let d = i as isize - 2;
                let rr = r as isize + dy * d;
                let cc = c as isize + dx * d;
                if rr < 0 || cc < 0 || rr >= h as isize || cc >= w as isize {
                    continue;
                }
                let v = src[rr as usize * w + cc as usize];
                if !v.is_nan() {
                    acc += kw * v;
                    weight += kw;
                }
            }
            if weight > 0.0 {
                out[r * w + c] = acc / weight;
            }
        }
    }
    out
}

/// Bilinear interpolation of the height at `(lat, lon)`. `None` outside the raster or without data.
pub fn height_at(r: &Raster, lat: f64, lon: f64) -> Option<f32> {
    let fc = (lon - r.lon0) / r.step_lon - 0.5;
    let fr = (r.lat0 - lat) / r.step_lat - 0.5;
    let c0 = fc.floor();
    let r0 = fr.floor();
    let tx = (fc - c0) as f32;
    let ty = (fr - r0) as f32;
    let (c0, r0) = (c0 as isize, r0 as isize);
    let (mut acc, mut weight) = (0.0f32, 0.0f32);
    for (dr, dc, pw) in [
        (0, 0, (1.0 - tx) * (1.0 - ty)),
        (0, 1, tx * (1.0 - ty)),
        (1, 0, (1.0 - tx) * ty),
        (1, 1, tx * ty),
    ] {
        let rr = r0 + dr;
        let cc = c0 + dc;
        if rr < 0 || cc < 0 || rr >= r.height as isize || cc >= r.width as isize {
            continue;
        }
        let v = r.values[rr as usize * r.width + cc as usize];
        if !v.is_nan() && pw > 0.0 {
            acc += pw * v;
            weight += pw;
        }
    }
    if weight > 0.0 {
        Some(acc / weight)
    } else {
        None
    }
}

/// Height at `(lat, lon)` from the first raster that has data there.
pub fn height_at_any(rasters: &[Raster], lat: f64, lon: f64) -> Option<f32> {
    rasters.iter().find_map(|r| height_at(r, lat, lon))
}

/// Samples the height grid of tile `(x, y)` (see the module doc).
pub fn tile_heights(r: &Raster, grid: &MapGrid, x: i32, y: i32) -> io::Result<HeightGrid> {
    tile_heights_any(std::slice::from_ref(r), grid, x, y)
}

/// Like `tile_heights`, with several rasters (the 1-degree DEM tiles that cover the map).
pub fn tile_heights_any(
    rasters: &[Raster],
    grid: &MapGrid,
    x: i32,
    y: i32,
) -> io::Result<HeightGrid> {
    let n = (grid.tile_m / grid.cell_m).round() as u32;
    let cols = n + 1;
    let x0 = x as f64 * grid.tile_m;
    let y0 = y as f64 * grid.tile_m;
    let mut values = Vec::with_capacity((cols * cols) as usize);
    for i in 0..cols {
        for j in 0..cols {
            let x_m = x0 + j as f64 * grid.cell_m;
            let y_m = y0 + i as f64 * grid.cell_m;
            let (lat, lon) = grid.latlon(x_m, y_m);
            let h = height_at_any(rasters, lat, lon).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no terrain data at tile ({x},{y}) point ({x_m:.1},{y_m:.1})"),
                )
            })?;
            values.push(h);
        }
    }
    Ok(HeightGrid {
        cols,
        rows: cols,
        values,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_home_tile_name() {
        let n = tile_name(crate::home::DEFAULT_LAT_DEG, crate::home::DEFAULT_LON_DEG);
        assert_eq!(n, "Copernicus_DSM_COG_10_S20_00_W044_00_DEM");
    }

    #[test]
    fn north_east_tile_name() {
        assert_eq!(
            tile_name(10.5, 5.2),
            "Copernicus_DSM_COG_10_N10_00_E005_00_DEM"
        );
    }

    /// 4x4 raster with `value = col * 1 + row * 10`, pixels of 0.01 degree.
    fn ramp() -> Raster {
        let (w, h) = (4usize, 4usize);
        let values = (0..h)
            .flat_map(|r| (0..w).map(move |c| c as f32 + 10.0 * r as f32))
            .collect();
        Raster {
            width: w,
            height: h,
            lon0: 0.0,
            lat0: 0.0,
            step_lon: 0.01,
            step_lat: 0.01,
            values,
        }
    }

    #[test]
    fn bilinear_at_pixel_center_and_midpoint() {
        let r = ramp();
        // pixel center (c=1, r=2): lon=0.015, lat=-0.025 -> value 1 + 20 = 21
        let v = height_at(&r, -0.025, 0.015).unwrap();
        assert!((v - 21.0).abs() < 1e-4, "{v}");
        // midpoint between (1,2) and (2,2): 21.5
        let v = height_at(&r, -0.025, 0.02).unwrap();
        assert!((v - 21.5).abs() < 1e-4, "{v}");
        // outside the raster
        assert!(height_at(&r, 1.0, 1.0).is_none());
    }

    #[test]
    fn tile_min_max_expected() {
        // Tilted flat DEM: h(lon, lat) = 1000 + 2*(lon/step) + 3*(-lat/step),
        // in pixel coordinates. Bilinear reproduces a plane exactly.
        // 20 m tile with a 10 m grid: 3x3 samples, corners at x,y in {0,10,20} m.
        let grid = MapGrid {
            lat0: 0.0,
            lon0: 0.0,
            tile_m: 20.0,
            cell_m: 10.0,
        };
        let (w, h) = (400usize, 400usize);
        let step = 0.0001;
        let (lon0, lat0) = (-0.01, 0.01); // raster covers the point (0,0) with margin
        let values: Vec<f32> = (0..h)
            .flat_map(|r| {
                (0..w).map(move |c| {
                    // pixel center: (lon - lon0)/step = c+0.5, (lat0 - lat)/step = r+0.5
                    let x = c as f64 + 0.5;
                    let y = r as f64 + 0.5;
                    (1000.0 + 2.0 * x + 3.0 * y) as f32
                })
            })
            .collect();
        let r = Raster {
            width: w,
            height: h,
            lon0,
            lat0,
            step_lon: step,
            step_lat: step,
            values,
        };
        let g = tile_heights(&r, &grid, 0, 0).unwrap();
        assert_eq!((g.cols, g.rows), (3, 3));
        // Expected: minimum at (x=0, y=+20 m) and maximum at (x=+20 m, y=0).
        let d = (20.0 / EARTH_RADIUS_M.to_radians()) / step; // 20 m in pixel steps
        let min_expected = 1000.0 + 2.0 * 100.0 + 3.0 * (100.0 - d);
        let max_expected = 1000.0 + 2.0 * (100.0 + d) + 3.0 * 100.0;
        let min = g.values.iter().cloned().fold(f32::INFINITY, f32::min) as f64;
        let max = g.values.iter().cloned().fold(f32::NEG_INFINITY, f32::max) as f64;
        assert!(
            (min - min_expected).abs() < 0.01,
            "min {min} vs {min_expected}"
        );
        assert!(
            (max - max_expected).abs() < 0.01,
            "max {max} vs {max_expected}"
        );
        // North is at a larger row: the value at corner (row=2, col=0) is the minimum.
        assert!((g.values[2 * 3] as f64 - min_expected).abs() < 0.01);
    }

    #[test]
    fn smoothing_reduces_peak_and_preserves_sum() {
        let (w, h) = (9usize, 9usize);
        let mut src = vec![0.0f32; w * h];
        src[4 * w + 4] = 100.0;
        let out = smooth(&src, w, h);
        let peak = out[4 * w + 4];
        // peak = (w0 * w0) * 100, with w0 = e^0 / sum(1 + 2e^-0.5 + 2e^-2)
        let sum = 1.0 + 2.0 * (-0.5f32).exp() + 2.0 * (-2.0f32).exp();
        let expected = (1.0 / sum) * (1.0 / sum) * 100.0;
        assert!((peak - expected).abs() < 1e-3, "{peak} vs {expected}");
        let total: f32 = out.iter().sum();
        assert!((total - 100.0).abs() < 1e-2, "{total}");
    }

    #[test]
    fn nodata_does_not_contaminate_neighbors() {
        let (w, h) = (5usize, 1usize);
        let src = vec![10.0, f32::NAN, 10.0, 10.0, 10.0];
        let out = smooth(&src, w, h);
        assert!(out[0].is_finite());
        assert!((out[0] - 10.0).abs() < 1e-4);
        assert!((out[2] - 10.0).abs() < 1e-4);
    }
}
