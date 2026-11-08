//! Distant terrain: plain DEM mesh (no buildings or collision) in square rings around the car,
//! beyond the detailed tiles, plus the road centerlines of the same area as flat strips on the
//! DEM (no collision either). Built on a background thread from the Copernicus rasters (read
//! once, smoothed the same way the tiles are baked) and from `roads.bin`.
//!
//! Layout: rings are centered on the car snapped to the tile lattice, so their edges fall on
//! grid lines of every ring. A cell whose center lies in a loaded tile is left out, because the
//! detailed terrain covers it; the cut edges and the outer edges get a skirt that hangs below the
//! surface, which hides the small height differences between the two sampling steps. The window
//! is rebuilt when the set of loaded tiles changes or the car moves `RECENTER_M` from its center.
//! Fog fades the far edge out.
//!
//! Decision on the source: the rings read the raw Copernicus GeoTIFFs at runtime (through
//! `tilebake::dem`, which is already a dependency), instead of a pre-baked LOD raster. Reading
//! costs about one second once, in the background; baking would need another format and a
//! second copy of the DEM on disk.
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use godot::classes::environment::FogMode;
use godot::classes::geometry_instance_3d::ShadowCastingSetting;
use godot::classes::mesh::{ArrayType, PrimitiveType};
use godot::classes::{
    ArrayMesh, Camera3D, Environment, GeometryInstance3D, Material, MeshInstance3D, Node, Node3D,
    StandardMaterial3D, WorldEnvironment,
};
use godot::prelude::*;
use tilebake::dem::{height_at_any, read_smoothed_dem, MapGrid, Raster};
use tileformat::roads::{read_roads_file, roads_path};
use tileformat::{road_width_m, MapMeta, Polyline};

use crate::geometry::MeshData;

/// One ring: a square window of `half_m` around the center, with cells of `step_m`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ring {
    pub half_m: f64,
    pub step_m: f64,
}

/// Inner to outer: 25 m cells to 2 km, 50 m to 5 km, 100 m to 10 km. Half-sizes are multiples of
/// the steps (and the 250 m tile), so the rings meet on grid lines.
pub const RINGS: [Ring; 3] = [
    Ring {
        half_m: 2000.0,
        step_m: 25.0,
    },
    Ring {
        half_m: 5000.0,
        step_m: 50.0,
    },
    Ring {
        half_m: 10000.0,
        step_m: 100.0,
    },
];
/// The windows are recentered once the car is this far from their center, in meters
/// (a quarter of the outer half-size).
pub const RECENTER_M: f64 = 2500.0;
/// How far the skirts hang below the surface, in meters.
const SKIRT_M: f32 = 30.0;
/// Fog: none up to `FOG_BEGIN_M`, opaque from `FOG_END_M` (hides the far edge of the rings).
pub(crate) const FOG_BEGIN_M: f32 = 5000.0;
pub(crate) const FOG_END_M: f32 = 10000.0;
/// Camera far plane, so the rings are not cut off before the fog takes them.
const FAR_M: f32 = 12000.0;
/// Fog color, the horizon color of the sky.
pub(crate) const FOG_COLOR: (f32, f32, f32) = (0.62, 0.82, 0.98);
const UP: [f32; 3] = [0.0, 1.0, 0.0];
/// Road strips sit this far above the DEM at their corners, so the coarse terrain cells between
/// the samples do not cover them.
const ROAD_LIFT_M: f32 = 0.5;
/// Spacing of the road points kept for the strips, in meters.
const ROAD_STEP_M: f64 = 40.0;
/// Road color, the same as the detailed roads (`earth_world.rs`).
const ROAD_RGB: (f32, f32, f32) = (0.22, 0.22, 0.24);

/// Terrain of one ring. Cells inside `inner_half_m` are left out (the inner ring has them) and
/// so are cells whose center lies in a tile of `loaded`. `height(x, y)` gives the height at local
/// `(x, y)` in meters, NaN where unknown; cells with an unknown corner are left out too.
pub fn build_ring(
    ring: Ring,
    inner_half_m: f64,
    center: [f64; 2],
    tile_m: f64,
    loaded: &HashSet<(i32, i32)>,
    height: &dyn Fn(f64, f64) -> f32,
) -> MeshData {
    let n = (2.0 * ring.half_m / ring.step_m).round() as usize;
    let m = n + 1;
    let x_at = |i: usize| center[0] - ring.half_m + i as f64 * ring.step_m;
    let y_at = |j: usize| center[1] - ring.half_m + j as f64 * ring.step_m;
    let h: Vec<f32> = (0..m * m)
        .map(|k| height(x_at(k % m), y_at(k / m)))
        .collect();

    // Cells [inner_lo, inner_hi) on each axis are the inner window.
    let inner_lo = ((ring.half_m - inner_half_m) / ring.step_m).round() as usize;
    let inner_hi = n - inner_lo;
    let inside_inner =
        |i: usize, j: usize| (inner_lo..inner_hi).contains(&i) && (inner_lo..inner_hi).contains(&j);
    let present: Vec<bool> = (0..n * n)
        .map(|c| {
            let (i, j) = (c % n, c / n);
            if inside_inner(i, j) {
                return false;
            }
            let xc = x_at(i) + ring.step_m / 2.0;
            let yc = y_at(j) + ring.step_m / 2.0;
            if loaded.contains(&((xc / tile_m).floor() as i32, (yc / tile_m).floor() as i32)) {
                return false;
            }
            [
                j * m + i,
                j * m + i + 1,
                (j + 1) * m + i,
                (j + 1) * m + i + 1,
            ]
            .iter()
            .all(|&k| !h[k].is_nan())
        })
        .collect();

    let mut mesh = MeshData {
        vertices: Vec::new(),
        normals: Vec::new(),
        colors: Vec::new(),
        uvs: Vec::new(),
        uv2: Vec::new(),
        indices: Vec::new(),
    };
    for k in 0..m * m {
        let (i, j) = (k % m, k / m);
        let y = if h[k].is_nan() { 0.0 } else { h[k] };
        mesh.vertices.push(world(x_at(i), y_at(j), y));
        mesh.normals.push(normal(&h, m, i, j, ring.step_m));
    }

    let present_at = |i: isize, j: isize| {
        i >= 0
            && j >= 0
            && (i as usize) < n
            && (j as usize) < n
            && present[j as usize * n + i as usize]
    };
    for j in 0..n {
        for i in 0..n {
            if !present_at(i as isize, j as isize) {
                continue;
            }
            let sw = (j * m + i) as u32;
            let (se, nw) = (sw + 1, sw + m as u32);
            let ne = nw + 1;
            push_facing(&mut mesh, sw, se, ne, UP);
            push_facing(&mut mesh, sw, ne, nw, UP);
            let (di, dj) = (i as isize, j as isize);
            if !present_at(di - 1, dj) {
                push_skirt(&mut mesh, sw, nw, [-1.0, 0.0]);
            }
            if !present_at(di + 1, dj) {
                push_skirt(&mut mesh, se, ne, [1.0, 0.0]);
            }
            if !present_at(di, dj - 1) {
                push_skirt(&mut mesh, sw, se, [0.0, -1.0]);
            }
            if !present_at(di, dj + 1) {
                push_skirt(&mut mesh, nw, ne, [0.0, 1.0]);
            }
        }
    }
    mesh
}

/// All rings around `center`, inner to outer.
pub fn build_rings(
    center: [f64; 2],
    tile_m: f64,
    loaded: &HashSet<(i32, i32)>,
    height: &dyn Fn(f64, f64) -> f32,
) -> Vec<MeshData> {
    let mut inner = 0.0;
    RINGS
        .iter()
        .map(|&ring| {
            let mesh = build_ring(ring, inner, center, tile_m, loaded, height);
            inner = ring.half_m;
            mesh
        })
        .collect()
}

/// Road strips of the roads outside the detailed tiles, inside the outer ring around `center`.
/// A segment is kept when one of its ends lies in an unloaded tile, so the strips meet the
/// detailed roads at the tile seam (they overlap a few meters there, no gap). Each segment is a
/// flat quad, `road_width_m` wide, at the DEM height under its corners plus `ROAD_LIFT_M`.
/// Points closer than `ROAD_STEP_M` to the last kept one are dropped (OSM nodes are ~2.5 m apart,
/// far finer than the rings), the last point of a piece is always kept.
pub fn build_roads(
    roads: &[(i32, i32, Vec<Polyline>)],
    center: [f64; 2],
    tile_m: f64,
    loaded: &HashSet<(i32, i32)>,
    height: &dyn Fn(f64, f64) -> f32,
) -> MeshData {
    let reach = RINGS[RINGS.len() - 1].half_m + tile_m;
    let in_window =
        |p: [f64; 2]| (p[0] - center[0]).abs() <= reach && (p[1] - center[1]).abs() <= reach;
    let unloaded = |p: [f64; 2]| {
        let tile = (
            (p[0] / tile_m).floor() as i32,
            (p[1] / tile_m).floor() as i32,
        );
        !loaded.contains(&tile)
    };
    let mut mesh = MeshData {
        vertices: Vec::new(),
        normals: Vec::new(),
        colors: Vec::new(),
        uvs: Vec::new(),
        uv2: Vec::new(),
        indices: Vec::new(),
    };
    for (_, _, pieces) in roads {
        for piece in pieces {
            let half = road_width_m(&piece.highway) / 2.0;
            let points = decimate(&piece.points, ROAD_STEP_M);
            for w in points.windows(2) {
                let (a, b) = (w[0], w[1]);
                if !(in_window(a) || in_window(b)) || !(unloaded(a) || unloaded(b)) {
                    continue;
                }
                push_segment(&mut mesh, a, b, half, height);
            }
        }
    }
    mesh
}

/// The points of a piece kept at least `step` meters apart, the last one always kept.
fn decimate(points: &[[f64; 2]], step: f64) -> Vec<[f64; 2]> {
    let mut kept: Vec<[f64; 2]> = Vec::with_capacity(points.len() / 4 + 2);
    let Some(&first) = points.first() else {
        return kept;
    };
    kept.push(first);
    for (k, &p) in points.iter().enumerate().skip(1) {
        let last = *kept.last().unwrap_or(&first);
        let d = (p[0] - last[0]).hypot(p[1] - last[1]);
        let is_end = k + 1 == points.len();
        if d >= step || (is_end && d > 1e-6) {
            kept.push(p);
        }
    }
    kept
}

/// One flat quad along the segment `a`-`b`, `half` meters to each side. Skipped when a corner
/// has no height.
fn push_segment(
    mesh: &mut MeshData,
    a: [f64; 2],
    b: [f64; 2],
    half: f64,
    height: &dyn Fn(f64, f64) -> f32,
) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dy);
    if len < 1e-6 {
        return;
    }
    let n = [-dy / len * half, dx / len * half];
    let corners = [
        [a[0] + n[0], a[1] + n[1]],
        [a[0] - n[0], a[1] - n[1]],
        [b[0] - n[0], b[1] - n[1]],
        [b[0] + n[0], b[1] + n[1]],
    ];
    let heights: Vec<f32> = corners.iter().map(|c| height(c[0], c[1])).collect();
    if heights.iter().any(|h| !h.is_finite()) {
        return;
    }
    let base = mesh.vertices.len() as u32;
    for (c, h) in corners.iter().zip(&heights) {
        mesh.vertices.push(world(c[0], c[1], h + ROAD_LIFT_M));
        mesh.normals.push(UP);
    }
    push_facing(mesh, base, base + 1, base + 2, UP);
    push_facing(mesh, base, base + 2, base + 3, UP);
}

/// Local `(x, y)` (east, north) plus height to Godot world space.
fn world(x: f64, y: f64, height: f32) -> [f32; 3] {
    [x as f32, height, -(y as f32)]
}

/// Normal from the slope of the grid at corner `(i, j)` (central differences, one-sided at the
/// border). Godot: x east, y up, z south, so the normal is `(-dh/dE, 1, dh/dN)`.
fn normal(h: &[f32], m: usize, i: usize, j: usize, step: f64) -> [f32; 3] {
    let (w, e) = (i.saturating_sub(1), (i + 1).min(m - 1));
    let (s, nn) = (j.saturating_sub(1), (j + 1).min(m - 1));
    let at = |a: usize, b: usize| h[b * m + a];
    let finite = [at(w, j), at(e, j), at(i, s), at(i, nn)]
        .iter()
        .all(|v| v.is_finite());
    let (dh_e, dh_n) = if finite {
        (
            (at(e, j) - at(w, j)) / ((e - w) as f64 * step) as f32,
            (at(i, nn) - at(i, s)) / ((nn - s) as f64 * step) as f32,
        )
    } else {
        (0.0, 0.0)
    };
    let v = [-dh_e, 1.0, dh_n];
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / len, v[1] / len, v[2] / len]
}

/// Adds the triangle, with its winding chosen so that its front faces `want`. Godot front faces
/// are clockwise seen from their normal, so the front normal is `-(b - a) x (c - a)`.
fn push_facing(mesh: &mut MeshData, a: u32, b: u32, c: u32, want: [f32; 3]) {
    let (pa, pb, pc) = (
        mesh.vertices[a as usize],
        mesh.vertices[b as usize],
        mesh.vertices[c as usize],
    );
    let u = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
    let v = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
    let cross = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let front = [-cross[0], -cross[1], -cross[2]];
    if front[0] * want[0] + front[1] * want[1] + front[2] * want[2] >= 0.0 {
        mesh.indices.extend([a, b, c]);
    } else {
        mesh.indices.extend([a, c, b]);
    }
}

/// Vertical strip from the edge `a`-`b` down by `SKIRT_M`, facing away from the window
/// (`dir` is the outward direction on the local plane).
fn push_skirt(mesh: &mut MeshData, a: u32, b: u32, dir: [f32; 2]) {
    let outward = [dir[0], 0.0, -dir[1]];
    let base = mesh.vertices.len() as u32;
    for &k in &[a, b] {
        let p = mesh.vertices[k as usize];
        mesh.vertices.push([p[0], p[1] - SKIRT_M, p[2]]);
        mesh.normals.push(outward);
    }
    let (ab, bb) = (base, base + 1);
    push_facing(mesh, a, b, bb, outward);
    push_facing(mesh, a, bb, ab, outward);
}

/// Work for the loader thread: the window to build and the tiles the detail covers.
struct Job {
    center: [f64; 2],
    loaded: HashSet<(i32, i32)>,
}

/// One ring (or the road strips) as the arrays of a Godot surface. Built on the loader thread
/// (plain data, so it can cross threads), copied into packed arrays on the main thread.
struct Surface {
    vertices: Vec<Vector3>,
    normals: Vec<Vector3>,
    indices: Vec<i32>,
    /// Road strips take the road material, the rings the terrain material.
    road: bool,
}

impl Surface {
    fn from_mesh(data: &MeshData, road: bool) -> Self {
        Surface {
            vertices: data.vertices.iter().map(|&p| vec3(p)).collect(),
            normals: data.normals.iter().map(|&p| vec3(p)).collect(),
            indices: data.indices.iter().map(|&i| i as i32).collect(),
            road,
        }
    }
}

/// Result of one build, from the loader thread.
struct Built {
    surfaces: Vec<Surface>,
    build_ms: f64,
}

/// Builds the terrain rings in the background, and turns them into a node on the main thread.
pub struct TerrainLod {
    /// Closed on drop, which ends the loader thread.
    jobs: Option<Sender<Job>>,
    results: Receiver<Result<Built, String>>,
    worker: Option<JoinHandle<()>>,
    /// Set on drop: the loader stops between DEM files and between builds.
    cancel: Arc<AtomicBool>,
    /// Center of the window that was last requested.
    center: Option<[f64; 2]>,
    /// Loaded tiles the last request was built for (sorted).
    hole: Vec<(i32, i32)>,
    in_flight: bool,
    /// The loader stopped after an error; no more requests.
    failed: bool,
    builds: u32,
    material: Gd<StandardMaterial3D>,
    road_material: Gd<StandardMaterial3D>,
}

impl TerrainLod {
    /// Starts the loader thread, which reads the Copernicus rasters of `<cache_root>/<id>/raw/copernicus`
    /// and the road pieces of `<cache_root>/<id>/roads.bin` (missing in old caches: no roads).
    pub fn start(
        cache_root: &Path,
        id: &str,
        meta: &MapMeta,
        material: Gd<StandardMaterial3D>,
    ) -> Self {
        let dir = cache_root.join(id).join("raw").join("copernicus");
        let roads_file = roads_path(cache_root, id);
        let grid = MapGrid::from_meta(meta);
        let tile_m = meta.tile_size_m;
        let (jobs, job_rx) = mpsc::channel::<Job>();
        let (res_tx, results) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker = thread::spawn(move || {
            let rasters = match load_rasters(&dir, &worker_cancel) {
                Ok(rasters) => rasters,
                Err(_) if worker_cancel.load(Ordering::Relaxed) => return,
                Err(e) => {
                    let _ = res_tx.send(Err(format!(
                        "cannot read DEM for terrain LOD in {}: {e}",
                        dir.display()
                    )));
                    return;
                }
            };
            let roads = match read_roads_file(&roads_file) {
                Ok(roads) => roads,
                Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
                Err(e) => {
                    let _ = res_tx.send(Err(format!(
                        "cannot read roads for terrain LOD in {}: {e}",
                        roads_file.display()
                    )));
                    return;
                }
            };
            for job in job_rx {
                if worker_cancel.load(Ordering::Relaxed) {
                    break;
                }
                let t0 = Instant::now();
                let height = |x: f64, y: f64| {
                    let (lat, lon) = grid.latlon(x, y);
                    height_at_any(&rasters, lat, lon).unwrap_or(f32::NAN)
                };
                let mut surfaces: Vec<Surface> =
                    build_rings(job.center, tile_m, &job.loaded, &height)
                        .iter()
                        .filter(|m| !m.is_empty())
                        .map(|m| Surface::from_mesh(m, false))
                        .collect();
                let strips = build_roads(&roads, job.center, tile_m, &job.loaded, &height);
                if !strips.is_empty() {
                    surfaces.push(Surface::from_mesh(&strips, true));
                }
                let built = Built {
                    surfaces,
                    build_ms: t0.elapsed().as_secs_f64() * 1000.0,
                };
                if res_tx.send(Ok(built)).is_err() {
                    break;
                }
            }
        });
        let mut road_material = StandardMaterial3D::new_gd();
        road_material.set_albedo(Color::from_rgb(ROAD_RGB.0, ROAD_RGB.1, ROAD_RGB.2));
        TerrainLod {
            jobs: Some(jobs),
            results,
            worker: Some(worker),
            cancel,
            center: None,
            hole: Vec::new(),
            in_flight: false,
            failed: false,
            builds: 0,
            material,
            road_material,
        }
    }

    /// Call once per frame with the car position and the loaded tiles. Requests a new build when
    /// the tiles or the window change, and returns the node of a finished build (the caller adds
    /// it and frees the previous one).
    pub fn update(
        &mut self,
        car: [f64; 2],
        loaded: &HashSet<(i32, i32)>,
    ) -> Option<Gd<MeshInstance3D>> {
        let mut ready = None;
        match self.results.try_recv() {
            Ok(Ok(built)) => {
                self.in_flight = false;
                self.builds += 1;
                let triangles: usize = built.surfaces.iter().map(|s| s.indices.len() / 3).sum();
                let t0 = Instant::now();
                let node = build_node(&built.surfaces, &self.material, &self.road_material);
                godot_print!(
                    "terrain LOD: build {} with {triangles} triangles, {:.1} ms in the loader, {:.1} ms on the main thread",
                    self.builds,
                    built.build_ms,
                    t0.elapsed().as_secs_f64() * 1000.0
                );
                ready = Some(node);
            }
            Ok(Err(e)) => {
                self.in_flight = false;
                self.failed = true;
                godot_error!("{e}");
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.in_flight = false;
                self.failed = true;
            }
        }
        if self.failed || self.in_flight {
            return ready;
        }

        let mut hole: Vec<(i32, i32)> = loaded.iter().copied().collect();
        hole.sort_unstable();
        let recenter = match self.center {
            None => true,
            Some(c) => (car[0] - c[0]).hypot(car[1] - c[1]) > RECENTER_M,
        };
        if recenter || hole != self.hole {
            if recenter {
                self.center = Some(snap(car));
            }
            self.hole = hole.clone();
            let job = Job {
                center: self.center.unwrap_or(car),
                loaded: loaded.clone(),
            };
            let sent = self
                .jobs
                .as_ref()
                .is_some_and(|jobs| jobs.send(job).is_ok());
            if sent {
                self.in_flight = true;
            } else {
                self.failed = true;
            }
        }
        ready
    }
}

impl Drop for TerrainLod {
    /// The loader reads the DEM files and must not run while the extension is unloaded, so
    /// this waits for it. It stops after the file it is reading, a few seconds at most.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.jobs = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Snaps a point to the tile lattice (250 m, a multiple of every ring step).
fn snap(p: [f64; 2]) -> [f64; 2] {
    const LATTICE_M: f64 = 250.0;
    [
        (p[0] / LATTICE_M).round() * LATTICE_M,
        (p[1] / LATTICE_M).round() * LATTICE_M,
    ]
}

/// Reads and smooths every GeoTIFF in `dir`. Stops with `Interrupted` once `cancel` is set.
fn load_rasters(dir: &Path, cancel: &AtomicBool) -> io::Result<Vec<Raster>> {
    let mut rasters = Vec::new();
    for entry in fs::read_dir(dir)? {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "tif") {
            rasters.push(read_smoothed_dem(&path)?);
        }
    }
    if rasters.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no .tif in the copernicus folder",
        ));
    }
    Ok(rasters)
}

/// One node with a surface per ring and one for the road strips. No collision: the detailed
/// tiles carry it.
fn build_node(
    surfaces: &[Surface],
    material: &Gd<StandardMaterial3D>,
    road_material: &Gd<StandardMaterial3D>,
) -> Gd<MeshInstance3D> {
    let mut mesh = ArrayMesh::new_gd();
    // One bulk copy per packed array: per-element pushes into Godot arrays cost a call each.
    for data in surfaces {
        let vertices = PackedVector3Array::from(data.vertices.as_slice());
        let normals = PackedVector3Array::from(data.normals.as_slice());
        let indices = PackedInt32Array::from(data.indices.as_slice());
        let mut arrays = VarArray::new();
        arrays.resize(ArrayType::MAX.ord() as usize, &Variant::nil());
        arrays.set(ArrayType::VERTEX.ord() as usize, &vertices.to_variant());
        arrays.set(ArrayType::NORMAL.ord() as usize, &normals.to_variant());
        arrays.set(ArrayType::INDEX.ord() as usize, &indices.to_variant());
        mesh.add_surface_from_arrays(PrimitiveType::TRIANGLES, &arrays);
        let own = if data.road { road_material } else { material };
        let index = mesh.get_surface_count() - 1;
        mesh.surface_set_material(index, &own.clone().upcast::<Material>());
    }
    let mut node = MeshInstance3D::new_alloc();
    node.set_mesh(&mesh);
    node.upcast_mut::<GeometryInstance3D>()
        .set_cast_shadows_setting(ShadowCastingSetting::OFF);
    node
}

fn vec3(p: [f32; 3]) -> Vector3 {
    Vector3::new(p[0], p[1], p[2])
}

/// Fog that fades the far edge of the rings, and a far plane for the car cameras.
/// `parent` is the scene root holding the `WorldEnvironment`; `car` holds the cameras.
pub fn configure_view(parent: Option<Gd<Node>>, car: &Gd<Node3D>) {
    let environment: Option<Gd<Environment>> = parent
        .and_then(|p| p.get_node_or_null("WorldEnvironment"))
        .and_then(|node| node.try_cast::<WorldEnvironment>().ok())
        .and_then(|w| w.get_environment());
    match environment {
        Some(mut env) => {
            env.set_fog_enabled(true);
            env.set_fog_mode(FogMode::DEPTH);
            env.set_fog_depth_begin(FOG_BEGIN_M);
            env.set_fog_depth_end(FOG_END_M);
            env.set_fog_light_color(Color::from_rgb(FOG_COLOR.0, FOG_COLOR.1, FOG_COLOR.2));
        }
        None => {
            godot_warn!("no WorldEnvironment under the scene root: distant terrain without fog");
        }
    }
    for child in car.get_children().iter_shared() {
        if let Ok(mut camera) = child.try_cast::<Camera3D>() {
            camera.set_far(FAR_M);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TILE: f64 = 250.0;

    fn flat(_: f64, _: f64) -> f32 {
        0.0
    }

    fn triangles(mesh: &MeshData) -> Vec<[[f32; 3]; 3]> {
        mesh.indices
            .chunks(3)
            .map(|t| {
                [
                    mesh.vertices[t[0] as usize],
                    mesh.vertices[t[1] as usize],
                    mesh.vertices[t[2] as usize],
                ]
            })
            .collect()
    }

    #[test]
    fn rings_meet_on_grid_lines() {
        // Every ring edge is a multiple of its own step, so neighboring rings line up. The two
        // inner rings also divide the tile size, so their cells never straddle a tile border.
        // The outer ring (100 m) is never under detailed tiles: the car stays within
        // RECENTER_M of the center and the detail reaches about 1.6 km from the car.
        for ring in RINGS {
            assert_eq!(ring.half_m % ring.step_m, 0.0, "{ring:?}");
        }
        for ring in &RINGS[..2] {
            assert_eq!(TILE % ring.step_m, 0.0, "{ring:?}");
        }
        assert_eq!(snap([-130.0, 390.0]), [-250.0, 500.0]);
    }

    #[test]
    fn full_ring_has_two_triangles_per_cell_and_skirts_on_the_edge() {
        let ring = Ring {
            half_m: 100.0,
            step_m: 25.0,
        };
        let mesh = build_ring(ring, 0.0, [0.0, 0.0], TILE, &HashSet::new(), &flat);
        let n = 8; // 200 m / 25 m
        assert_eq!(mesh.indices.len() / 3, 2 * n * n + 2 * 4 * n);
    }

    #[test]
    fn loaded_tile_is_left_out_with_a_skirt_around_it() {
        let ring = Ring {
            half_m: 2000.0,
            step_m: 25.0,
        };
        let loaded: HashSet<(i32, i32)> = [(0, 0)].into_iter().collect();
        let mesh = build_ring(ring, 0.0, [0.0, 0.0], TILE, &loaded, &flat);
        // Tile (0, 0) is 250 m, 10 cells of 25 m per side: 100 cells out, 40 new skirt edges.
        let n = 160;
        assert_eq!(mesh.indices.len() / 3, 2 * (n * n - 100) + 2 * (4 * n + 40));
    }

    #[test]
    fn inner_window_is_left_out() {
        let outer = Ring {
            half_m: 200.0,
            step_m: 50.0,
        };
        let mesh = build_ring(outer, 100.0, [0.0, 0.0], TILE, &HashSet::new(), &flat);
        // 8x8 cells minus the 4x4 inside the inner window, plus skirts on both boundaries.
        assert_eq!(mesh.indices.len() / 3, 2 * (64 - 16) + 2 * (4 * 8 + 4 * 4));
    }

    #[test]
    fn unknown_heights_leave_cells_out() {
        let ring = Ring {
            half_m: 100.0,
            step_m: 50.0,
        };
        let hole = |x: f64, _: f64| if x > 0.0 { f32::NAN } else { 0.0 };
        let mesh = build_ring(ring, 0.0, [0.0, 0.0], TILE, &HashSet::new(), &hole);
        assert!(mesh
            .indices
            .iter()
            .all(|&i| !mesh.vertices[i as usize][1].is_nan()));
        // Corners at x <= 0 are known: two columns of cells (x in [-100, 0]), four rows. That is
        // 8 cells, and 12 skirt edges around them (4 west, 4 east, 2 south, 2 north).
        assert_eq!(mesh.indices.len() / 3, 2 * 8 + 2 * 12);
    }

    #[test]
    fn front_faces_up_on_flat_ground() {
        let ring = Ring {
            half_m: 100.0,
            step_m: 25.0,
        };
        let mesh = build_ring(ring, 0.0, [0.0, 0.0], TILE, &HashSet::new(), &flat);
        for t in triangles(&mesh) {
            let (a, b, c) = (t[0], t[1], t[2]);
            if a[1] != 0.0 || b[1] != 0.0 || c[1] != 0.0 {
                continue; // skirts
            }
            let cross_y = (b[2] - a[2]) * (c[0] - a[0]) - (b[0] - a[0]) * (c[2] - a[2]);
            assert!(cross_y < 0.0, "{t:?} is back-facing for Godot");
        }
    }

    fn road_piece(points: Vec<[f64; 2]>) -> Polyline {
        Polyline {
            oneway: None,
            lanes: None,
            maxspeed: None,
            roundabout: false,
            highway: "residential".into(),
            name: String::new(),
            points,
            z: Vec::new(),
            zl: Vec::new(),
            zr: Vec::new(),
            lit: None,
        }
    }

    #[test]
    fn roads_inside_loaded_tiles_are_left_out_and_seams_are_kept() {
        // Tile (0, 0) (x in [0, 250)) is loaded. Segments with both ends in it are left out; a
        // segment with one end in an unloaded tile is kept, so the strip reaches the seam.
        let loaded: HashSet<(i32, i32)> = [(0, 0)].into_iter().collect();
        let roads = vec![(
            0,
            0,
            vec![road_piece(vec![
                [100.0, 0.0],
                [200.0, 0.0],
                [300.0, 0.0],
                [400.0, 0.0],
            ])],
        )];
        let mesh = build_roads(&roads, [0.0, 0.0], TILE, &loaded, &flat);
        // Segments: 100-200 (both in tile 0, dropped), 200-300 (kept, 300 unloaded), 300-400 (kept).
        assert_eq!(mesh.indices.len() / 3, 2 * 2);
        assert!(mesh.vertices.iter().all(|v| v[1] > 0.0));
    }

    #[test]
    fn road_strips_skip_unknown_heights_and_the_outer_window() {
        let far = vec![(
            0,
            0,
            vec![road_piece(vec![[20_000.0, 0.0], [20_100.0, 0.0]])],
        )];
        let mesh = build_roads(&far, [0.0, 0.0], TILE, &HashSet::new(), &flat);
        assert!(mesh.is_empty());
        let hole = |x: f64, _: f64| if x > 300.0 { f32::NAN } else { 0.0 };
        let near = vec![(
            0,
            0,
            vec![road_piece(vec![[100.0, 0.0], [200.0, 0.0], [400.0, 0.0]])],
        )];
        let mesh = build_roads(&near, [0.0, 0.0], TILE, &HashSet::new(), &hole);
        // 100-200 is known; 200-400 has an unknown corner at x > 300 and is dropped.
        assert_eq!(mesh.indices.len() / 3, 2);
    }

    #[test]
    fn decimate_keeps_spacing_and_the_last_point() {
        let dense: Vec<[f64; 2]> = (0..=20).map(|k| [k as f64 * 2.5, 0.0]).collect(); // 0..50 m
        let kept = decimate(&dense, 10.0);
        // 0, 10, 20, 30, 40 (spacing >= 10), then 50, the last point, which is 10 m after 40.
        assert_eq!(kept.len(), 6);
        assert_eq!(kept.last(), Some(&[50.0, 0.0]));
        // A short last step is still kept, so the piece reaches its end.
        let short = vec![[0.0, 0.0], [10.0, 0.0], [12.0, 0.0]];
        assert_eq!(decimate(&short, 10.0).last(), Some(&[12.0, 0.0]));
    }

    #[test]
    fn normals_point_away_from_a_rise_and_up() {
        // Height rises to the east: the normal leans west (negative x), and is always up.
        let slope = |x: f64, _: f64| (x * 0.1) as f32;
        let ring = Ring {
            half_m: 100.0,
            step_m: 25.0,
        };
        let mesh = build_ring(ring, 0.0, [0.0, 0.0], TILE, &HashSet::new(), &slope);
        let mid = mesh.normals[2 * 9 + 2]; // interior corner
        assert!(mid[0] < 0.0 && mid[1] > 0.0);
        assert!((mid[0] * mid[0] + mid[1] * mid[1] + mid[2] * mid[2] - 1.0).abs() < 1e-5);
    }
}
