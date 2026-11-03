//! S0 study: horizon profile from the home over the 10 km Copernicus DEM cache, and the sunset of
//! 15/11/2026 in Belo Horizonte.
//!
//! For each compass azimuth (0 = north, clockwise) from `AZ_FROM` to `AZ_TO`, the terrain is sampled
//! every `STEP_D_M` up to `MAX_D_M` from the home. The eye is 1.5 m above the ground at home. The
//! crest is the sample with the highest angular altitude seen from the eye; the peak is the highest
//! terrain sample on the ray. Run: `cargo run --release -p tilebake --example horizon`.
use std::f64::consts::PI;
use std::fs;
use std::path::PathBuf;

use tilebake::dem::{height_at_any, read_smoothed_dem, Raster};

const HOME_LAT: f64 = -19.9006792;
const HOME_LON: f64 = -43.9174041;
const EYE_M: f64 = 1.5;
const MAX_D_M: f64 = 15_000.0;
const STEP_D_M: f64 = 50.0;
const AZ_FROM: f64 = 90.0;
const AZ_TO: f64 = 200.0;
const AZ_STEP: f64 = 2.0;
const PRINT_EVERY_DEG: f64 = 10.0;
/// A ridge counts as "visible" when its crest is at least this high above the eye, in degrees.
const RIDGE_MIN_DEG: f64 = 2.0;
const EARTH_RADIUS_M: f64 = 6_371_000.0;
/// 15 November 2026 is day 319 of the year.
const SUN_DAY_OF_YEAR: f64 = 319.0;
/// Belo Horizonte: UTC-3, no daylight saving time.
const TZ_HOURS: f64 = -3.0;

#[derive(Clone, Copy)]
struct Sample {
    az: f64,
    dist_m: f64,
    elev_m: f64,
    ang_deg: f64,
}

/// Sunset in local hours, and its azimuth (compass degrees), by the Spencer declination and
/// equation-of-time series, evaluated at the day's mean (not exact) position.
fn sunset(lat: f64, lon: f64, day: f64, tz: f64) -> (f64, f64) {
    let g = 2.0 * PI / 365.0 * (day - 1.0);
    let decl = 0.006918 - 0.399912 * g.cos() + 0.070257 * g.sin() - 0.006758 * (2.0 * g).cos()
        + 0.000907 * (2.0 * g).sin()
        - 0.002697 * (3.0 * g).cos()
        + 0.00148 * (3.0 * g).sin();
    // Equation of time, minutes.
    let eqt = 229.18
        * (0.000075 + 0.001868 * g.cos() - 0.032077 * g.sin() - 0.014615 * (2.0 * g).cos()
            - 0.040849 * (2.0 * g).sin());
    let phi = lat.to_radians();
    let zenith = 90.833_f64.to_radians();
    let cos_ha = (zenith.cos() - phi.sin() * decl.sin()) / (phi.cos() * decl.cos());
    let ha_deg = cos_ha.acos().to_degrees();
    let noon_utc_min = 720.0 - 4.0 * lon - eqt;
    let set_utc_min = noon_utc_min + 4.0 * ha_deg;
    let set_local_h = (set_utc_min / 60.0 + tz).rem_euclid(24.0);
    let alt = (-0.833_f64).to_radians();
    let cos_az = (decl.sin() - phi.sin() * alt.sin()) / (phi.cos() * alt.cos());
    let az = 360.0 - cos_az.acos().to_degrees();
    (set_local_h, az)
}

fn hm(h: f64) -> String {
    let m = (h * 60.0).round() as i64;
    format!("{:02}:{:02}", m / 60, m % 60)
}

fn main() {
    let dir: PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", "..", "cache", "casa-10km", "raw", "copernicus"]
        .iter()
        .collect();
    let mut tifs: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("read copernicus cache dir")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "tif"))
        .collect();
    tifs.sort();
    let rasters: Vec<Raster> = tifs
        .iter()
        .map(|p| read_smoothed_dem(p).expect("read DEM tif"))
        .collect();
    println!("DEM rasters: {}", rasters.len());

    let ground = height_at_any(&rasters, HOME_LAT, HOME_LON).expect("no DEM at home") as f64;
    let eye = ground + EYE_M;
    println!("home ground {ground:.1} m, eye {eye:.1} m");

    let m_lat = EARTH_RADIUS_M.to_radians();
    let m_lon = m_lat * HOME_LAT.to_radians().cos();
    let steps = (MAX_D_M / STEP_D_M).round() as usize;
    let n_az = ((AZ_TO - AZ_FROM) / AZ_STEP).round() as usize + 1;
    let mut nodata = 0usize;
    let mut rows: Vec<(Sample, Sample)> = Vec::with_capacity(n_az);

    for i in 0..n_az {
        let az = AZ_FROM + i as f64 * AZ_STEP;
        let (sin, cos) = az.to_radians().sin_cos();
        let empty = Sample { az, dist_m: 0.0, elev_m: f64::NEG_INFINITY, ang_deg: f64::NEG_INFINITY };
        let (mut crest, mut peak) = (empty, empty);
        for k in 1..=steps {
            let d = k as f64 * STEP_D_M;
            let lat = HOME_LAT + d * cos / m_lat;
            let lon = HOME_LON + d * sin / m_lon;
            let Some(h) = height_at_any(&rasters, lat, lon) else {
                nodata += 1;
                continue;
            };
            let h = h as f64;
            let ang = ((h - eye) / d).atan().to_degrees();
            if ang > crest.ang_deg {
                crest = Sample { az, dist_m: d, elev_m: h, ang_deg: ang };
            }
            if h > peak.elev_m {
                peak = Sample { az, dist_m: d, elev_m: h, ang_deg: ang };
            }
        }
        rows.push((crest, peak));
    }

    println!();
    println!("az   | crista: dist km  elev m  ang°  | pico: dist km  elev m  ang°");
    let every = (PRINT_EVERY_DEG / AZ_STEP).round() as usize;
    for (i, (c, p)) in rows.iter().enumerate() {
        if i % every != 0 {
            continue;
        }
        println!(
            "{:>4.0} | {:>13.2} {:>7.0} {:>6.2} | {:>12.2} {:>7.0} {:>6.2}",
            c.az,
            c.dist_m / 1000.0,
            c.elev_m,
            c.ang_deg,
            p.dist_m / 1000.0,
            p.elev_m,
            p.ang_deg
        );
    }

    let hc = rows
        .iter()
        .map(|r| r.0)
        .fold(rows[0].0, |acc, c| if c.ang_deg > acc.ang_deg { c } else { acc });
    let highest_peak = rows
        .iter()
        .map(|r| r.1)
        .fold(rows[0].1, |acc, p| if p.elev_m > acc.elev_m { p } else { acc });
    let nearest = rows
        .iter()
        .map(|r| r.0)
        .filter(|c| c.ang_deg >= RIDGE_MIN_DEG)
        .fold(None::<Sample>, |acc, c| match acc {
            Some(a) if a.dist_m <= c.dist_m => Some(a),
            _ => Some(c),
        });
    let max_visible_dist = rows
        .iter()
        .map(|r| r.0)
        .filter(|c| c.ang_deg >= RIDGE_MIN_DEG)
        .map(|c| c.dist_m)
        .fold(0.0, f64::max);
    let visible = rows.iter().filter(|r| r.0.ang_deg >= RIDGE_MIN_DEG).count();

    println!();
    println!("crista de maior angulo: az {:.0}, dist {:.2} km, elev {:.0} m, {:.2}°", hc.az, hc.dist_m / 1000.0, hc.elev_m, hc.ang_deg);
    println!("pico mais alto no setor: az {:.0}, dist {:.2} km, elev {:.0} m, {:.2}°", highest_peak.az, highest_peak.dist_m / 1000.0, highest_peak.elev_m, highest_peak.ang_deg);
    match nearest {
        Some(n) => println!("crista mais proxima (>= {RIDGE_MIN_DEG}°): az {:.0}, dist {:.2} km, elev {:.0} m, {:.2}°", n.az, n.dist_m / 1000.0, n.elev_m, n.ang_deg),
        None => println!("crista mais proxima: nenhuma acima de {RIDGE_MIN_DEG}°"),
    }
    println!("azimutes com crista >= {RIDGE_MIN_DEG}°: {visible} de {n_az}; distancia max da crista visivel: {:.2} km", max_visible_dist / 1000.0);
    println!("amostras sem DEM: {nodata}");

    let (set_h, set_az) = sunset(HOME_LAT, HOME_LON, SUN_DAY_OF_YEAR, TZ_HOURS);
    println!();
    println!("por do sol 15/11/2026 BH: ocaso {} (local), azimute {set_az:.1}°", hm(set_h));

    // Optional: `horizon <file>` with one "lat lon" pair per line; prints ground stats and centroid.
    if let Some(path) = std::env::args().nth(1) {
        points_stats(&rasters, &path, m_lat, m_lon);
    }
}

/// Ground height of the points in `path` ("lat lon" per line): count, mean, min, max, centroid, and
/// the centroid's distance and compass azimuth from home.
fn points_stats(rasters: &[Raster], path: &str, m_lat: f64, m_lon: f64) {
    let text = fs::read_to_string(path).expect("read points file");
    let (mut n, mut sum, mut min, mut max) = (0usize, 0.0f64, f64::INFINITY, f64::NEG_INFINITY);
    let (mut clat, mut clon) = (0.0f64, 0.0f64);
    for line in text.lines() {
        let mut it = line.split_whitespace().map(|v| v.parse::<f64>().expect("lat/lon"));
        let (Some(lat), Some(lon)) = (it.next(), it.next()) else { continue };
        clat += lat;
        clon += lon;
        let Some(h) = height_at_any(rasters, lat, lon) else { continue };
        let h = h as f64;
        n += 1;
        sum += h;
        min = min.min(h);
        max = max.max(h);
    }
    let total = text.lines().count().max(1) as f64;
    let (clat, clon) = (clat / total, clon / total);
    let (dx, dy) = ((clon - HOME_LON) * m_lon, (clat - HOME_LAT) * m_lat);
    let dist = dx.hypot(dy);
    let az = dx.atan2(dy).to_degrees().rem_euclid(360.0);
    println!();
    println!("pontos {total:.0} (com DEM {n}): elev media {:.0} m, min {min:.0} m, max {max:.0} m", sum / n.max(1) as f64);
    println!("centroide {clat:.5} {clon:.5}: dist {:.2} km, az {az:.1}°", dist / 1000.0);
}
