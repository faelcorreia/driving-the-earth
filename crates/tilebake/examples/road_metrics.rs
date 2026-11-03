//! F10 validation on the road profile of a baked map (`cache/<id>/tiles`): the ribbon follows
//! the baked `z` of each road piece, so the metrics below are taken from that profile at
//! 2.5 m steps (the ribbon's station spacing), within 500 m of the home (map origin).
//! Reports: pitch change between consecutive 2.5 m steps (cabeceio, deg), height kink
//! (second difference, cm), lumps (std of the height minus its 20 m moving average, mm) on
//! straight and curved stretches, and the first curve with a horizontal turn > 20 deg within
//! 1.5 km (found automatically). Seams and tile_geometry timing are checked in gdext.
//! Run: `cargo run --release -p tilebake --example road_metrics -- [map-id-or-dir]`.
use std::fs;
use std::path::{Path, PathBuf};

use tileformat::{read_tile, Polyline};

const STEP: f64 = 2.5;
const HOME_M: f64 = 500.0;
const CURVE_SEARCH_M: f64 = 1500.0;

/// One road piece resampled every `STEP` meters: (x, y, height).
fn resample(road: &Polyline) -> Vec<[f64; 3]> {
    let z = &road.z;
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
            let h = z[k] as f64 + (z[k + 1] - z[k]) as f64 * t;
            out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, h]);
            s += STEP;
        }
        carry = len - (s - STEP);
    }
    out
}

fn heading(p: [f64; 3], q: [f64; 3]) -> f64 {
    (q[1] - p[1]).atan2(q[0] - p[0])
}

fn turn_deg(s: &[[f64; 3]], i: usize, k: usize) -> f64 {
    let d = (heading(s[i - k], s[i]) - heading(s[i], s[i + k])).abs();
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

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "casa".into());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cache");
    let dir: PathBuf = if Path::new(&arg).is_dir() { arg.into() } else { root.join(&arg) };
    let mut pitch = (vec![], vec![]); // (straight, curve)
    let mut kink = (0.0f64, 0usize);
    let mut lumps = (vec![], vec![]);
    let mut first_curve: Option<(f64, f64, Vec<f64>)> = None;
    let (mut pieces, mut without_z) = (0, 0);
    let mut worst: Vec<(f64, [f64; 2], usize, usize)> = vec![]; // (pitch, pos, piece, step)
    let mut no_z_dist: Vec<f64> = vec![];
    for entry in fs::read_dir(dir.join("tiles")).expect("tiles dir") {
        let path = entry.expect("entry").path();
        let tile = read_tile(&path).expect("tile");
        for road in &tile.roads {
            if road.z.len() != road.points.len() {
                without_z += 1;
                no_z_dist.push(road.points.iter().map(|p| p[0].hypot(p[1])).fold(f64::MAX, f64::min));
                continue;
            }
            let s = resample(road);
            pieces += 1;
            if s.len() < 12 {
                continue;
            }
            let hs: Vec<f64> = s.iter().map(|p| p[2]).collect();
            let near = |p: &[f64; 3]| p[0].hypot(p[1]) <= HOME_M;
            for i in 4..s.len() - 4 {
                let turn = turn_deg(&s, i, 4);
                if !near(&s[i]) {
                    continue;
                }
                let ang = |j: usize| ((hs[j + 1] - hs[j]) / STEP).atan().to_degrees();
                let dp = (ang(i) - ang(i - 1)).abs();
                let bucket = if turn > 10.0 { &mut pitch.1 } else { &mut pitch.0 };
                worst.push((dp, [s[i][0], s[i][1]], pieces, i));
                bucket.push(dp);
                let k2 = (hs[i + 1] - 2.0 * hs[i] + hs[i - 1]).abs();
                kink = (kink.0.max(k2), kink.1 + 1);
            }
            for i in 10..s.len() - 10 {
                if !near(&s[i]) {
                    continue;
                }
                let ma = hs[i - 8..=i + 8].iter().sum::<f64>() / 17.0;
                let r = (hs[i] - ma) * 1000.0;
                if turn_deg(&s, i, 4) < 5.0 {
                    lumps.0.push(r);
                } else if turn_deg(&s, i, 4) > 10.0 {
                    lumps.1.push(r);
                }
            }
            if first_curve.is_none() {
                for i in 6..s.len() - 6 {
                    let t = turn_deg(&s, i, 6);
                    let d = s[i][0].hypot(s[i][1]);
                    if t > 20.0 && d <= CURVE_SEARCH_M {
                        let seg: Vec<f64> = (i - 4..=i + 4).map(|j| (hs[j + 1] - hs[j]) / STEP).collect();
                        first_curve = Some((d, t, seg));
                        break;
                    }
                }
            }
        }
    }
    worst.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let edge = |x: f64| (x / 250.0 - (x / 250.0).round()).abs() * 250.0;
    println!("10 worst pitch steps (deg, pos, dist to tile line m, piece, step index):");
    for (dp, pos, piece, i) in worst.iter().take(10) {
        let d_tile = edge(pos[0]).min(edge(pos[1]));
        println!("  {dp:6.2}  ({:7.1},{:7.1})  seam {d_tile:6.1}  piece {piece:4}  step {i}", pos[0], pos[1]);
    }
    let mut nd = no_z_dist.clone();
    let inside = nd.iter().filter(|d| **d <= 1000.0).count();
    println!("no-z pieces: {} with a point within 1000 m of home; min-dist pct p50 {:.0} p95 {:.0} max {:.0} m", inside, pct(&mut nd, 0.5), pct(&mut nd, 0.95), pct(&mut nd, 1.0));
    let (mut ps, mut pc) = (pitch.0, pitch.1);
    println!("road pieces with profile: {pieces} (without z: {without_z})");
    println!("pitch 2.5 m steps, straight: max {:.3} p95 {:.3} deg ({} samples)", ps.iter().cloned().fold(0.0, f64::max), pct(&mut ps, 0.95), ps.len());
    println!("pitch 2.5 m steps, curve:    max {:.3} p95 {:.3} deg ({} samples)", pc.iter().cloned().fold(0.0, f64::max), pct(&mut pc, 0.95), pc.len());
    println!("height kink (2nd difference, 2.5 m): max {:.2} cm", kink.0 * 100.0);
    println!("lumps (height - 20 m mean): straight std {:.1} mm, curve std {:.1} mm", std(&lumps.0), std(&lumps.1));
    match first_curve {
        Some((d, t, seg)) => println!(
            "curve with turn {:.1} deg at {:.0} m from home: slope steps (%) {:?}",
            t,
            d,
            seg.iter().map(|g| (g * 1000.0).round() / 10.0).collect::<Vec<_>>()
        ),
        None => println!("no curve with turn > 20 deg within {CURVE_SEARCH_M} m"),
    }
}
