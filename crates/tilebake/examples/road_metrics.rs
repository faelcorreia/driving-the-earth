//! F10 validation on the road profile of a baked map (`cache/<id>/tiles`): the ribbon follows
//! the baked `z` of each road piece, so the metrics below are taken from that profile at
//! 2.5 m steps (the ribbon's station spacing), within 500 m of the home (map origin).
//! Reports: pitch change between consecutive 2.5 m steps (cabeceio, deg), height kink
//! (second difference, cm), lumps (std of the height minus its 20 m moving average, mm) on
//! straight and curved stretches, and the first curve with a horizontal turn > 20 deg within
//! 1.5 km (found automatically).
//! Diagnosis: the 15 worst kinks and the 15 worst pitches (one per location, at least 5 steps
//! apart) are matched to the raw OSM way (same `highway`, nearest) and classified by cause.
//! For each, the window of +-6 stations shows `z` (profile), `T` (baked terrain grid, the
//! terrain before the profile), `L` and `R` (zl and zr minus z), all relative to the station.
//! Seams and tile_geometry timing are checked in gdext.
//! Run: `cargo run --release -p tilebake --example road_metrics -- [map-id-or-dir]`.
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use tilebake::dem::MapGrid;
use tilebake::osm::visit_ways;
use tilebake::profile::node_key;
use tileformat::{read_tile, MapMeta, Polyline, Tile};

const STEP: f64 = 2.5;
const HOME_M: f64 = 500.0;
const CURVE_SEARCH_M: f64 = 1500.0;
const WIN: usize = 6;
const TOP: usize = 15;
const CAUSES: [&str; 8] = [
    "ponte/tunel",
    "cruzamento niveis",
    "costura tile",
    "corte piso/teto",
    "no compartilhado",
    "quina polilinha",
    "DEM estrutura",
    "ruido DEM",
];

/// One station of a road piece: position, profile height and the two edge heights.
#[derive(Clone, Copy)]
struct Station {
    p: [f64; 2],
    z: f64,
    zl: f64,
    zr: f64,
}

/// One road piece resampled every `STEP` meters.
fn resample(road: &Polyline) -> Vec<Station> {
    let z = &road.z;
    let has_edges = road.zl.len() == z.len() && road.zr.len() == z.len();
    let mut out = vec![];
    let mut carry = 0.0;
    for k in 0..road.points.len().saturating_sub(1) {
        let (a, b) = (road.points[k], road.points[k + 1]);
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        if len <= 1e-9 {
            continue;
        }
        let mut s = STEP - carry;
        while s <= len {
            let t = s / len;
            let lerp = |v: &[f32]| v[k] as f64 + (v[k + 1] - v[k]) as f64 * t;
            out.push(Station {
                p: [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t],
                z: lerp(z),
                zl: if has_edges { lerp(&road.zl) } else { f64::NAN },
                zr: if has_edges { lerp(&road.zr) } else { f64::NAN },
            });
            s += STEP;
        }
        carry = len - (s - STEP);
    }
    out
}

fn heading(p: [f64; 2], q: [f64; 2]) -> f64 {
    (q[1] - p[1]).atan2(q[0] - p[0])
}

fn turn_deg(s: &[Station], i: usize, k: usize) -> f64 {
    let d = (heading(s[i - k].p, s[i].p) - heading(s[i].p, s[i + k].p)).abs();
    d.min(2.0 * std::f64::consts::PI - d).to_degrees()
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p) as usize]
}

fn std(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let m = v.iter().sum::<f64>() / v.len() as f64;
    (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
}

/// A raw OSM highway way in the local plane, with the tags the diagnosis needs.
struct Way {
    id: i64,
    highway: String,
    bridge: String,
    tunnel: String,
    layer: i64,
    junction: String,
    pts: Vec<[f64; 2]>,
}

impl Way {
    fn is_structure(&self) -> bool {
        !matches!(self.bridge.as_str(), "" | "no") || !matches!(self.tunnel.as_str(), "" | "no")
    }
}

fn load_ways(dir: &Path, meta: &MapMeta) -> Vec<Way> {
    let grid = MapGrid::from_meta(meta);
    let mut ways = vec![];
    visit_ways(&dir.join("raw").join("osm"), meta, |e| {
        let Some(hw) = e["tags"]["highway"].as_str() else {
            return;
        };
        let pts: Vec<[f64; 2]> = e["geometry"]
            .as_array()
            .map(|g| {
                g.iter()
                    .filter_map(|p| {
                        let (x, y) = grid.local(p["lat"].as_f64()?, p["lon"].as_f64()?);
                        Some([x, y])
                    })
                    .collect()
            })
            .unwrap_or_default();
        if pts.len() < 2 {
            return;
        }
        let tag = |k: &str| e["tags"][k].as_str().unwrap_or("").to_string();
        ways.push(Way {
            id: e["id"].as_i64().unwrap_or(-1),
            highway: hw.to_string(),
            bridge: tag("bridge"),
            tunnel: tag("tunnel"),
            layer: tag("layer").parse().unwrap_or(0),
            junction: tag("junction"),
            pts,
        });
    })
    .expect("raw osm");
    ways
}

/// Number of distinct ways that have each (rounded) OSM vertex: 2 or more means a junction.
fn node_counts(ways: &[Way]) -> HashMap<(i64, i64), usize> {
    let mut m = HashMap::new();
    for w in ways {
        let keys: HashSet<(i64, i64)> = w.pts.iter().map(|p| node_key(*p)).collect();
        for k in keys {
            *m.entry(k).or_insert(0) += 1;
        }
    }
    m
}

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
}

fn dist_to_way(p: [f64; 2], w: &Way) -> f64 {
    w.pts
        .windows(2)
        .map(|s| seg_dist(p, s[0], s[1]))
        .fold(f64::MAX, f64::min)
}

/// Distance from `p` to the nearest tile line (m).
fn seam(p: [f64; 2], tm: f64) -> f64 {
    let e = |x: f64| (x / tm - (x / tm).round()).abs() * tm;
    e(p[0]).min(e(p[1]))
}

/// Baked terrain grid of `tile` (bilinear) at `p`: the terrain before the road profile.
fn grid_h(tile: &Tile, tm: f64, p: [f64; 2]) -> f64 {
    let h = &tile.height;
    let n = h.cols as usize;
    let cell = tm / (n - 1) as f64;
    let fx = ((p[0] - tile.x as f64 * tm) / cell).clamp(0.0, (n - 1) as f64);
    let fy = ((p[1] - tile.y as f64 * tm) / cell).clamp(0.0, (n - 1) as f64);
    let (j, i) = (
        (fx.floor() as usize).min(n - 2),
        (fy.floor() as usize).min(n - 2),
    );
    let (u, v) = (fx - j as f64, fy - i as f64);
    let at = |r: usize, c: usize| h.values[r * n + c] as f64;
    at(i, j) * (1.0 - u) * (1.0 - v)
        + at(i, j + 1) * u * (1.0 - v)
        + at(i + 1, j) * (1.0 - u) * v
        + at(i + 1, j + 1) * u * v
}

/// A metric sample: value (deg or m), the file, the road index in the tile, the station index.
struct Hit {
    val: f64,
    path: PathBuf,
    road: usize,
    i: usize,
}

/// Prints one diagnosed location and returns its cause.
#[allow(clippy::too_many_arguments)]
fn report(
    label: &str,
    val_txt: &str,
    path: &Path,
    tile: &Tile,
    road: &Polyline,
    s: &[Station],
    i: usize,
    tm: f64,
    ways: &[Way],
    nodes: &HashMap<(i64, i64), usize>,
) -> &'static str {
    let st = s[i];
    let p = st.p;
    let dseam = seam(p, tm);
    let best = ways
        .iter()
        .enumerate()
        .filter(|(_, w)| w.highway == road.highway)
        .map(|(k, w)| (dist_to_way(p, w), k))
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let way = best.map(|(_, k)| &ways[k]);
    let cross = ways.iter().enumerate().find(|(k, w)| {
        Some(*k) != best.map(|b| b.1)
            && way.is_some_and(|m| w.layer != m.layer)
            && dist_to_way(p, w) < 4.0
    });
    let (dnode, vtx) = ways
        .iter()
        .flat_map(|w| w.pts.iter())
        .map(|q| ((q[0] - p[0]).hypot(q[1] - p[1]), *q))
        .fold((f64::MAX, p), |a, b| if b.0 < a.0 { b } else { a });
    let shared = nodes.get(&node_key(vtx)).copied().unwrap_or(0);
    let lo = i.saturating_sub(WIN);
    let hi = (i + WIN).min(s.len() - 1);
    let terr: Vec<f64> = (lo..=hi).map(|j| grid_h(tile, tm, s[j].p)).collect();
    let trange =
        terr.iter().fold(f64::MIN, |a, b| a.max(*b)) - terr.iter().fold(f64::MAX, |a, b| a.min(*b));
    let t_here = grid_h(tile, tm, p);
    let d = st.z - t_here;
    let clamp = (d - 0.5).abs() < 0.01 || (d + 0.15).abs() < 0.01;
    let corner = i >= 4 && i + 4 < s.len() && turn_deg(s, i, 4) > 15.0;
    let tagged = way.is_some_and(|w| w.is_structure());
    let cause = if tagged {
        "ponte/tunel"
    } else if cross.is_some() {
        "cruzamento niveis"
    } else if dseam < 3.0 {
        "costura tile"
    } else if clamp {
        "corte piso/teto"
    } else if dnode < 3.0 && shared >= 2 {
        "no compartilhado"
    } else if corner {
        "quina polilinha"
    } else if trange > 1.0 {
        "DEM estrutura"
    } else {
        "ruido DEM"
    };
    let fmt_tag = |w: Option<&Way>, f: fn(&Way) -> String| w.map(f).unwrap_or_else(|| "-".into());
    let cross_s = cross
        .map(|(_, w)| format!("{}@L{}", w.id, w.layer))
        .unwrap_or_else(|| "-".into());
    println!(
        "[{label}] {val_txt} {} tile({},{}) hw={} at({:.1},{:.1}) i={} seam={:.1} dnode={:.2} shared={} way={} bridge={} tunnel={} layer={} junction={} cross={} trange={:.2}m z-T={:.1}cm -> {cause}",
        path.file_name().unwrap().to_string_lossy(),
        tile.x,
        tile.y,
        road.highway,
        p[0],
        p[1],
        i,
        dseam,
        dnode,
        shared,
        fmt_tag(way, |w| w.id.to_string()),
        fmt_tag(way, |w| if w.bridge.is_empty() { "-".into() } else { w.bridge.clone() }),
        fmt_tag(way, |w| if w.tunnel.is_empty() { "-".into() } else { w.tunnel.clone() }),
        fmt_tag(way, |w| w.layer.to_string()),
        fmt_tag(way, |w| if w.junction.is_empty() { "-".into() } else { w.junction.clone() }),
        cross_s,
        trange,
        d * 100.0,
    );
    let rel = |f: &dyn Fn(usize) -> f64, base: f64| -> String {
        (lo..=hi)
            .map(|j| format!("{:+.1}", (f(j) - base) * 100.0))
            .collect::<Vec<_>>()
            .join(" ")
    };
    println!("     z [cm] {}", rel(&|j| s[j].z, st.z));
    println!("     T [cm] {}", rel(&|j| grid_h(tile, tm, s[j].p), t_here));
    println!("     L [cm] {}", rel(&|j| s[j].zl - s[j].z, st.zl - st.z));
    println!("     R [cm] {}", rel(&|j| s[j].zr - s[j].z, st.zr - st.z));
    cause
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "casa".into());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache");
    let dir: PathBuf = if Path::new(&arg).is_dir() {
        arg.into()
    } else {
        root.join(&arg)
    };
    let meta: MapMeta =
        serde_json::from_str(&fs::read_to_string(dir.join("meta.json")).expect("meta.json"))
            .expect("meta json");
    let tm = meta.tile_size_m;
    let mut pitch = (vec![], vec![]); // (straight, curve)
    let mut kink = (0.0f64, 0usize);
    let mut lumps = (vec![], vec![]);
    let mut first_curve: Option<(f64, f64, Vec<f64>)> = None;
    let (mut pieces, mut without_z) = (0, 0);
    let mut kinks: Vec<Hit> = vec![];
    let mut pitches: Vec<Hit> = vec![];
    for entry in fs::read_dir(dir.join("tiles")).expect("tiles dir") {
        let path = entry.expect("entry").path();
        let tile = read_tile(&path).expect("tile");
        for (ri, road) in tile.roads.iter().enumerate() {
            if road.z.len() != road.points.len() {
                without_z += 1;
                continue;
            }
            let s = resample(road);
            pieces += 1;
            if s.len() < 12 {
                continue;
            }
            let hs: Vec<f64> = s.iter().map(|st| st.z).collect();
            let near = |p: [f64; 2]| p[0].hypot(p[1]) <= HOME_M;
            let ang = |j: usize| ((hs[j + 1] - hs[j]) / STEP).atan().to_degrees();
            for i in 4..s.len() - 4 {
                if !near(s[i].p) {
                    continue;
                }
                let turn = turn_deg(&s, i, 4);
                let dp = (ang(i) - ang(i - 1)).abs();
                if turn > 10.0 {
                    pitch.1.push(dp);
                } else {
                    pitch.0.push(dp);
                }
                pitches.push(Hit {
                    val: dp,
                    path: path.clone(),
                    road: ri,
                    i,
                });
                let k2 = (hs[i + 1] - 2.0 * hs[i] + hs[i - 1]).abs();
                kink = (kink.0.max(k2), kink.1 + 1);
                kinks.push(Hit {
                    val: k2,
                    path: path.clone(),
                    road: ri,
                    i,
                });
            }
            for i in 10..s.len() - 10 {
                if !near(s[i].p) {
                    continue;
                }
                let ma = hs[i - 8..=i + 8].iter().sum::<f64>() / 17.0;
                let r = (hs[i] - ma) * 1000.0;
                let t = turn_deg(&s, i, 4);
                if t < 5.0 {
                    lumps.0.push(r);
                } else if t > 10.0 {
                    lumps.1.push(r);
                }
            }
            if first_curve.is_none() {
                for i in 6..s.len() - 6 {
                    let t = turn_deg(&s, i, 6);
                    let d = s[i].p[0].hypot(s[i].p[1]);
                    if t > 20.0 && d <= CURVE_SEARCH_M {
                        let seg: Vec<f64> = (i - 4..=i + 4)
                            .map(|j| (hs[j + 1] - hs[j]) / STEP)
                            .collect();
                        first_curve = Some((d, t, seg));
                        break;
                    }
                }
            }
        }
    }
    let (mut ps, mut pc) = (pitch.0, pitch.1);
    println!("road pieces with profile: {pieces} (without z: {without_z})");
    println!(
        "pitch 2.5 m steps, straight: max {:.3} p95 {:.3} deg ({} samples)",
        ps.iter().cloned().fold(0.0, f64::max),
        pct(&mut ps, 0.95),
        ps.len()
    );
    println!(
        "pitch 2.5 m steps, curve:    max {:.3} p95 {:.3} deg ({} samples)",
        pc.iter().cloned().fold(0.0, f64::max),
        pct(&mut pc, 0.95),
        pc.len()
    );
    println!(
        "height kink (2nd difference, 2.5 m): max {:.2} cm",
        kink.0 * 100.0
    );
    println!(
        "lumps (height - 20 m mean): straight std {:.1} mm, curve std {:.1} mm",
        std(&lumps.0),
        std(&lumps.1)
    );
    match first_curve {
        Some((d, t, seg)) => println!(
            "curve with turn {:.1} deg at {:.0} m from home: slope steps (%) {:?}",
            t,
            d,
            seg.iter()
                .map(|g| (g * 1000.0).round() / 10.0)
                .collect::<Vec<_>>()
        ),
        None => println!("no curve with turn > 20 deg within {CURVE_SEARCH_M} m"),
    }

    // Diagnosis of the worst locations.
    let ways = load_ways(&dir, &meta);
    let nodes = node_counts(&ways);
    let select = |mut v: Vec<Hit>| -> Vec<Hit> {
        v.sort_by(|a, b| b.val.total_cmp(&a.val));
        let mut out: Vec<Hit> = vec![];
        for h in v {
            if out.len() >= TOP {
                break;
            }
            if out.iter().any(|o| {
                o.path == h.path && o.road == h.road && (o.i as i64 - h.i as i64).abs() <= 5
            }) {
                continue;
            }
            out.push(h);
        }
        out
    };
    let top_k = select(kinks);
    let top_p = select(pitches);
    let mut tally: Vec<(&str, usize, usize)> = CAUSES.iter().map(|c| (*c, 0, 0)).collect();
    for (label, list, is_kink) in [("K", &top_k, true), ("P", &top_p, false)] {
        println!(
            "--- worst {} ({}) ---",
            if is_kink { "kinks" } else { "pitches" },
            list.len()
        );
        for (rank, h) in list.iter().enumerate() {
            let tile = read_tile(&h.path).expect("tile");
            let road = &tile.roads[h.road];
            let s = resample(road);
            let val_txt = if is_kink {
                format!("{:.2}cm", h.val * 100.0)
            } else {
                format!("{:.2}deg", h.val)
            };
            let cause = report(
                &format!("{label}{:02}", rank + 1),
                &val_txt,
                &h.path,
                &tile,
                road,
                &s,
                h.i,
                tm,
                &ways,
                &nodes,
            );
            let idx = CAUSES.iter().position(|c| *c == cause).unwrap();
            if is_kink {
                tally[idx].1 += 1;
            } else {
                tally[idx].2 += 1;
            }
        }
    }
    println!("--- causes (of the 15 worst): kinks | pitches ---");
    for (c, k, p) in tally {
        println!(
            "  {c:<18} {k:2} ({:3.0}%) | {p:2} ({:3.0}%)",
            k as f64 * 100.0 / top_k.len().max(1) as f64,
            p as f64 * 100.0 / top_p.len().max(1) as f64
        );
    }
}
