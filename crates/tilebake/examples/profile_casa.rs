//! F12 validation: height profile along the street of the home (`cache/casa`), read from the baked
//! tiles, against an independent reference: the raw contour segments crossed by the street polyline.
//! The street comes from the OSM cache (`cache/casa/raw/overpass.json`), not from code.
//! The name is the first argument, or the `street` field of `home.local.json`.
//! Run: `cargo run --release -p tilebake --example profile_casa [street name]`.
use std::fs;
use std::path::Path;

use tilebake::dem::MapGrid;
use tilebake::home::home;
use tilebake::shp::read_contours;
use tilebake::utm::{inverse, utm23s, UTM23S_LON0};
use tileformat::{read_tile, tile_path, MapMeta, Tile};

const HALF_M: f64 = 200.0;
const STEP_M: f64 = 10.0;

/// Bilinear height of the tile grid at local `(x, y)` (east, north), in meters.
fn height_in(tile: &Tile, x: f64, y: f64, tile_m: f64) -> f64 {
    let cols = tile.height.cols as usize;
    let cell = tile_m / (cols - 1) as f64;
    let fx = (x - tile.x as f64 * tile_m) / cell;
    let fy = (y - tile.y as f64 * tile_m) / cell;
    let c0 = (fx.floor() as usize).min(cols - 2);
    let r0 = (fy.floor() as usize).min(cols - 2);
    let (tx, ty) = (fx - c0 as f64, fy - r0 as f64);
    let v = |r: usize, c: usize| tile.height.values[r * cols + c] as f64;
    (1.0 - ty) * ((1.0 - tx) * v(r0, c0) + tx * v(r0, c0 + 1))
        + ty * ((1.0 - tx) * v(r0 + 1, c0) + tx * v(r0 + 1, c0 + 1))
}

/// Intersection of segments `p-q` and `r-s`: parameters `(t on pq, u on rs)` when they cross.
fn cross(p: [f64; 2], q: [f64; 2], r: [f64; 2], s: [f64; 2]) -> Option<(f64, f64)> {
    let d = (q[0] - p[0]) * (s[1] - r[1]) - (q[1] - p[1]) * (s[0] - r[0]);
    if d.abs() < 1e-12 {
        return None;
    }
    let t = ((r[0] - p[0]) * (s[1] - r[1]) - (r[1] - p[1]) * (s[0] - r[0])) / d;
    let u = ((r[0] - p[0]) * (q[1] - p[1]) - (r[1] - p[1]) * (q[0] - p[0])) / d;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some((t, u))
}

fn main() {
    let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache");
    let meta: MapMeta =
        serde_json::from_str(&fs::read_to_string(cache.join("casa/meta.json")).expect("meta"))
            .expect("meta json");
    let grid = MapGrid::from_meta(&meta);
    let tile_m = meta.tile_size_m;
    let (home_lat, home_lon) = (meta.center_lat, meta.center_lon);
    let street = std::env::args()
        .nth(1)
        .or_else(|| home().street.clone())
        .expect("street: pass the name as argument or set \"street\" in home.local.json");

    // Street polyline (UTM), nearest way with that name to the home.
    let overpass: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(cache.join("casa/raw/overpass.json")).expect("overpass"),
    )
    .expect("json");
    let (he, hn) = utm23s(home_lat, home_lon);
    let mut best: Option<(f64, Vec<[f64; 2]>)> = None;
    for el in overpass["elements"].as_array().expect("elements") {
        if el["type"] != "way" || el["tags"]["name"] != street {
            continue;
        }
        let line: Vec<[f64; 2]> = el["geometry"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let (e, n) = utm23s(p["lat"].as_f64().unwrap(), p["lon"].as_f64().unwrap());
                [e, n]
            })
            .collect();
        for w in line.windows(2) {
            let (dx, dy) = (w[1][0] - w[0][0], w[1][1] - w[0][1]);
            let t =
                (((he - w[0][0]) * dx + (hn - w[0][1]) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
            let d = (w[0][0] + t * dx - he).hypot(w[0][1] + t * dy - hn);
            if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                best = Some((d, line.clone()));
            }
        }
    }
    let (_, line) = best.expect("street not in cache");
    // Cumulative arc length along the street; home projected onto it.
    let mut cum = vec![0.0];
    for w in line.windows(2) {
        cum.push(cum.last().unwrap() + (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]));
    }
    let pos = |a: f64| -> [f64; 2] {
        let a = a.clamp(0.0, *cum.last().unwrap());
        let k = cum.partition_point(|&c| c <= a).clamp(1, line.len() - 1) - 1;
        let t = if cum[k + 1] > cum[k] {
            (a - cum[k]) / (cum[k + 1] - cum[k])
        } else {
            0.0
        };
        [
            line[k][0] + t * (line[k + 1][0] - line[k][0]),
            line[k][1] + t * (line[k + 1][1] - line[k][1]),
        ]
    };
    let mut a_home = 0.0;
    let mut dmin = f64::MAX;
    for (k, w) in line.windows(2).enumerate() {
        let (dx, dy) = (w[1][0] - w[0][0], w[1][1] - w[0][1]);
        let t = (((he - w[0][0]) * dx + (hn - w[0][1]) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        let d = (w[0][0] + t * dx - he).hypot(w[0][1] + t * dy - hn);
        if d < dmin {
            dmin = d;
            a_home = cum[k] + t * (cum[k + 1] - cum[k]);
        }
    }
    // Positive s points south (decreasing north), as in the reference of the street.
    let sgn = if pos(a_home + 1.0)[1] < pos(a_home - 1.0)[1] {
        1.0
    } else {
        -1.0
    };
    let arc_of = |s: f64| a_home + sgn * s;

    // Reference: crossings of the street with the raw contour segments, (s, z), sorted by s.
    let src = cache.join("regions-src/br-mg-belo-horizonte");
    let mut crossings: Vec<(f64, f64)> = Vec::new();
    for shp in [
        "curva_nivel_mestra_ano-2015.shp",
        "curva_nivel_secundaria_ano-2015.shp",
    ] {
        let path = src.join(shp);
        let mut segs: Vec<([f64; 2], [f64; 2], f64)> = Vec::new();
        read_contours(
            &path,
            &path.with_extension("dbf"),
            "COTA_CURVA",
            |ax, ay, bx, by, z| {
                let near = |x: f64, y: f64| {
                    (x - he).abs() < HALF_M + 50.0 && (y - hn).abs() < HALF_M + 50.0
                };
                if near(ax, ay) || near(bx, by) {
                    segs.push(([ax, ay], [bx, by], z));
                }
            },
        )
        .expect("contours");
        for (k, w) in line.windows(2).enumerate() {
            for (p, q, z) in &segs {
                if let Some((t, u)) = cross(w[0], w[1], *p, *q) {
                    let a = cum[k] + t * (cum[k + 1] - cum[k]);
                    let _ = u;
                    crossings.push(((a - a_home) * sgn, *z));
                }
            }
        }
    }
    crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
    let reference = |s: f64| -> Option<f64> {
        let i = crossings.partition_point(|c| c.0 <= s);
        if i == 0 || i >= crossings.len() {
            return None;
        }
        let ((s0, z0), (s1, z1)) = (crossings[i - 1], crossings[i]);
        Some(if s1 > s0 {
            z0 + (z1 - z0) * (s - s0) / (s1 - s0)
        } else {
            z0
        })
    };

    // Surface from the baked tiles at the same points.
    let mut tiles = std::collections::HashMap::new();
    let n = (HALF_M / STEP_M) as i32;
    let mut rows = Vec::new();
    for k in -n..=n {
        let s = k as f64 * STEP_M;
        let p = pos(arc_of(s));
        let (lat, lon) = inverse(p[0], p[1], UTM23S_LON0);
        let (x, y) = grid.local(lat, lon);
        let key = ((x / tile_m).floor() as i32, (y / tile_m).floor() as i32);
        let tile = tiles.entry(key).or_insert_with(|| {
            read_tile(&tile_path(&cache, "casa", key.0, key.1)).expect("tile missing")
        });
        rows.push((s, height_in(tile, x, y, tile_m), reference(s)));
    }
    println!("s_m (+ south)  baked_m  ref_m  err_m");
    let (mut max_e, mut sum_e, mut cnt) = (0.0f64, 0.0f64, 0usize);
    for &(s, h, r) in &rows {
        match r {
            Some(r) => {
                println!("{s:7.0}  {h:8.2}  {r:8.2}  {:+6.2}", h - r);
                max_e = max_e.max((h - r).abs());
                sum_e += (h - r).abs();
                cnt += 1;
            }
            None => println!("{s:7.0}  {h:8.2}      -       -"),
        }
    }
    println!("error vs contour reference: max {max_e:.2} m, mean {:.2} m over {cnt} points ({} crossings)", sum_e / cnt.max(1) as f64, crossings.len());
    let at = |s: f64| {
        rows.iter()
            .find(|r| (r.0 - s).abs() < 1e-9)
            .map(|r| (r.1, r.2))
    };
    for (a, b) in [
        (-200.0, -100.0),
        (-100.0, 0.0),
        (0.0, 100.0),
        (100.0, 200.0),
    ] {
        if let (Some((ha, ra)), Some((hb, rb))) = (at(a), at(b)) {
            let slope = |x0: f64, x1: f64| (x0 - x1) / (b - a) * 100.0;
            println!(
                "slope {a:>5}..{b:>4} m: baked {:.1}% | ref {}",
                slope(ha, hb),
                match (ra, rb) {
                    (Some(x), Some(y)) => format!("{:.1}%", slope(x, y)),
                    _ => "n/a".into(),
                }
            );
        }
    }
}
