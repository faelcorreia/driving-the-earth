//! Downloads only the areas, nature and street infrastructure layer of one cache entry, and
//! prints the counts and the Ribeirão Arrudas waterway (if any):
//! `cargo run --release -p tilebake --example download_areas -- <cache-root> <id>`
//! e.g. `... -- /mnt/Data/Codes/driving-the-earth/cache casa-10km`.
use std::path::Path;
use std::process::exit;
use std::time::Instant;

use tilebake::osm_areas;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(root), Some(id)) = (args.next(), args.next()) else {
        eprintln!("usage: download_areas <cache-root> <id>");
        exit(2);
    };
    let started = Instant::now();
    let s = match tilebake::download_osm_areas(Path::new(&root), &id) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            exit(1);
        }
    };
    println!(
        "{id}: areas blocks {} downloaded {} retries {} written {:.1} MB, on disk {:.1} MB, in {:.1} s",
        s.blocks,
        s.blocks_downloaded,
        s.retries,
        s.bytes_written as f64 / 1e6,
        s.areas_bytes as f64 / 1e6,
        started.elapsed().as_secs_f64()
    );
    for (t, n) in &s.counts.by_type {
        println!("elements type {t}: {n}");
    }
    for (c, n) in &s.counts.by_category {
        println!("category {c}: {n}");
    }
    // Ribeirão Arrudas: waterway ways named after it, with their node count.
    let dir = Path::new(&root).join(&id).join("raw").join(osm_areas::DIR_NAME);
    let mut arrudas = Vec::new();
    let res = osm_areas::visit_elements(&dir, &s.meta, |e| {
        let name = e["tags"]["name"].as_str().unwrap_or("");
        if e["tags"]["waterway"].is_string() && name.contains("Arrudas") {
            let nodes = e["geometry"].as_array().map_or(0, |g| g.len());
            arrudas.push((name.to_string(), e["id"].as_u64().unwrap_or(0), nodes));
        }
    });
    if let Err(e) = res {
        eprintln!("error reading the layer: {e}");
        exit(1);
    }
    if arrudas.is_empty() {
        println!("Arrudas: not found");
    }
    for (name, id, nodes) in arrudas {
        println!("Arrudas: way {id} name={name} nodes={nodes}");
    }
}
