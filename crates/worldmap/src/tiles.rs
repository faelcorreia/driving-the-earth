//! Vector tiles for the home picker (F17b3): slippy tile ids on the unit square of `projection`
//! (the same Web Mercator as the view), the zoom to use for a view, and decoding of OpenMapTiles
//! MVT (served by OpenFreeMap) into roads, road names and neighborhood names. Pure: no HTTP here,
//! see `fetch`. Coordinates of the decoded features are on the unit square.
use std::collections::HashSet;

use geo_types::Geometry;
use mvt_reader::feature::Value;
use mvt_reader::Reader;

use crate::cull::bbox_of;
use crate::view::View;

/// Highest zoom with data in OpenFreeMap's planet tiles. Deeper views stretch these tiles.
pub const MAX_ZOOM: u8 = 14;
/// A view uses the deepest zoom whose tiles cover the screen with at most this many tiles.
pub const MAX_TILES_PER_VIEW: usize = 24;
/// Below this zoom (doublings above the whole world) the picker asks for no tiles.
pub const DETAIL_MIN_STEPS: f64 = 5.0;
/// Minor streets (and service roads, tracks) are drawn from this zoom on.
pub const MINOR_MIN_STEPS: f64 = 8.0;
/// When a covering tile is not loaded yet, its parent (then grandparent...) is drawn instead, up to
/// this many zooms up, so the screen is not blank while the detail loads.
pub const FALLBACK_LEVELS: u8 = 3;

/// Road classes kept from the `transportation` layer.
pub const ROAD_CLASSES: [&str; 8] = [
    "motorway",
    "trunk",
    "primary",
    "secondary",
    "tertiary",
    "minor",
    "service",
    "track",
];
/// Place classes drawn from tiles. Cities and towns come from the embedded dataset instead.
pub const PLACE_CLASSES: [&str; 4] = ["quarter", "suburb", "neighbourhood", "hamlet"];

/// A slippy tile: zoom and column/row in the 2^z grid of the unit square.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileId {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

/// A decoded line or point, with its class and name (empty name if none).
#[derive(Debug, Clone, PartialEq)]
pub struct Feature {
    pub class: String,
    pub name: String,
    /// Unit square points: a line for roads, one point for places.
    pub pts: Vec<[f64; 2]>,
    /// Unit square box of `pts`, for culling without projecting every point.
    pub bbox: [f64; 4],
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TileData {
    pub roads: Vec<Feature>,
    /// Name lines of roads (`transportation_name`); the name runs along the line.
    pub road_names: Vec<Feature>,
    pub places: Vec<Feature>,
}

/// Whether a road class is a main road (drawn at every zoom with data).
pub fn is_major_class(class: &str) -> bool {
    matches!(
        class,
        "motorway" | "trunk" | "primary" | "secondary" | "tertiary"
    )
}

/// Zoom for a view: the deepest zoom (up to `MAX_ZOOM`) whose covering tiles fit in
/// `MAX_TILES_PER_VIEW`. `None` when the view is too far out to show tile data.
pub fn zoom_for_view(view: &View) -> Option<u8> {
    if view.zoom_steps() < DETAIL_MIN_STEPS {
        return None;
    }
    let mut best = 0;
    for z in 0..=MAX_ZOOM {
        if count_in_view(view, z) > MAX_TILES_PER_VIEW {
            break;
        }
        best = z;
    }
    Some(best)
}

/// Unit square bounds of the screen: (x0, y0, x1, y1).
fn bounds(view: &View) -> (f64, f64, f64, f64) {
    let (x0, y0) = view.to_unit(0.0, 0.0);
    let (x1, y1) = view.to_unit(view.width, view.height);
    (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))
}

/// Inclusive index range of the grid cells that a unit interval touches.
fn index_range(lo: f64, hi: f64, n: i64) -> (i64, i64) {
    (
        (lo * n as f64).floor() as i64,
        (hi * n as f64).floor() as i64,
    )
}

fn count_in_view(view: &View, z: u8) -> usize {
    let n = 1i64 << z;
    let (x0, y0, x1, y1) = bounds(view);
    let (ax, bx) = index_range(x0, x1, n);
    let (ay, by) = index_range(y0, y1, n);
    let cols = (bx - ax + 1).max(0) as usize;
    let rows = (by.min(n - 1) - ay.max(0) + 1).max(0) as usize;
    cols * rows
}

/// The parent tile (one zoom up).
pub fn parent(t: TileId) -> TileId {
    TileId {
        z: t.z - 1,
        x: t.x / 2,
        y: t.y / 2,
    }
}

/// Tiles to draw for a view at zoom `z`: each covering tile that `has` (is loaded), or else its
/// nearest loaded ancestor within `FALLBACK_LEVELS`. Each tile is listed once.
pub fn display_tiles<F: Fn(&TileId) -> bool>(view: &View, z: u8, has: F) -> Vec<TileId> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for t in cover(view, z) {
        let mut cand = t;
        loop {
            if has(&cand) {
                if seen.insert(cand) {
                    out.push(cand);
                }
                break;
            }
            if cand.z == 0 || z - cand.z >= FALLBACK_LEVELS {
                break;
            }
            cand = parent(cand);
        }
    }
    out
}

/// Tiles to keep in memory while a view at zoom `z` is shown: the covering tiles, their 8
/// neighbours (so a small pan finds them loaded) and their ancestors within `FALLBACK_LEVELS`.
pub fn keep_set(view: &View, z: u8) -> HashSet<TileId> {
    let n = 1i64 << z;
    let mut out = HashSet::new();
    for t in cover(view, z) {
        for dy in -1i64..=1 {
            for dx in -1i64..=1 {
                let y = t.y as i64 + dy;
                if y < 0 || y >= n {
                    continue;
                }
                let x = (t.x as i64 + dx).rem_euclid(n);
                out.insert(TileId {
                    z,
                    x: x as u32,
                    y: y as u32,
                });
            }
        }
        let mut anc = t;
        for _ in 0..FALLBACK_LEVELS {
            if anc.z == 0 {
                break;
            }
            anc = parent(anc);
            out.insert(anc);
        }
    }
    out
}

/// Tiles of zoom `z` that cover the screen, nearest to the screen center first. Columns wrap
/// around the antimeridian; rows outside the map are skipped.
pub fn cover(view: &View, z: u8) -> Vec<TileId> {
    let n = 1i64 << z;
    let (x0, y0, x1, y1) = bounds(view);
    let (ax, bx) = index_range(x0, x1, n);
    let (ay, by) = index_range(y0, y1, n);
    let (cx, cy) = view.to_unit(view.width / 2.0, view.height / 2.0);
    let mut out = Vec::new();
    for y in ay.max(0)..=by.min(n - 1) {
        for x in ax..=bx {
            out.push(TileId {
                z,
                x: x.rem_euclid(n) as u32,
                y: y as u32,
            });
        }
    }
    let dist = |t: &TileId| {
        let s = (1i64 << t.z) as f64;
        let (tx, ty) = ((t.x as f64 + 0.5) / s, (t.y as f64 + 0.5) / s);
        (tx - cx).powi(2) + (ty - cy).powi(2)
    };
    out.sort_by(|a, b| dist(a).total_cmp(&dist(b)));
    out.dedup();
    out
}

fn prop_str(props: &Option<std::collections::HashMap<String, Value>>, key: &str) -> String {
    match props.as_ref().and_then(|p| p.get(key)) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// The lines (or points) of a geometry, in tile coordinates.
fn parts(g: &Geometry<f32>) -> Vec<Vec<(f64, f64)>> {
    let c = |c: &geo_types::Coord<f32>| (c.x as f64, c.y as f64);
    match g {
        Geometry::LineString(l) => vec![l.0.iter().map(c).collect()],
        Geometry::MultiLineString(m) => m.0.iter().map(|l| l.0.iter().map(c).collect()).collect(),
        Geometry::Point(p) => vec![vec![(p.x() as f64, p.y() as f64)]],
        Geometry::MultiPoint(m) => {
            m.0.iter()
                .map(|p| vec![(p.x() as f64, p.y() as f64)])
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Decodes one MVT tile (uncompressed protobuf, as OpenFreeMap serves it). Layers other than
/// `transportation`, `transportation_name` and `place` are ignored, and so are classes not listed
/// above. Fails only if the bytes are not a vector tile.
pub fn decode(id: TileId, bytes: &[u8]) -> Result<TileData, String> {
    let reader = Reader::new(bytes.to_vec()).map_err(|e| format!("{e:?}"))?;
    let names = reader.get_layer_names().map_err(|e| format!("{e:?}"))?;
    let metas = reader.get_layer_metadata().map_err(|e| format!("{e:?}"))?;
    let n = (1u64 << id.z) as f64;
    let mut out = TileData::default();
    for (i, layer) in names.iter().enumerate() {
        if !matches!(
            layer.as_str(),
            "transportation" | "transportation_name" | "place"
        ) {
            continue;
        }
        let extent = metas
            .get(i)
            .map(|m| m.extent as f64)
            .filter(|e| *e > 0.0)
            .unwrap_or(4096.0);
        let features = reader.get_features(i).map_err(|e| format!("{e:?}"))?;
        for f in features {
            let class = prop_str(&f.properties, "class");
            let name = prop_str(&f.properties, "name");
            let target = match layer.as_str() {
                "transportation" if ROAD_CLASSES.contains(&class.as_str()) => &mut out.roads,
                "transportation_name" if !name.is_empty() => &mut out.road_names,
                "place" if PLACE_CLASSES.contains(&class.as_str()) && !name.is_empty() => {
                    &mut out.places
                }
                _ => continue,
            };
            for part in parts(&f.geometry) {
                let pts: Vec<[f64; 2]> = part
                    .iter()
                    .map(|&(px, py)| {
                        [
                            (id.x as f64 + px / extent) / n,
                            (id.y as f64 + py / extent) / n,
                        ]
                    })
                    .collect();
                if !pts.is_empty() {
                    let bbox = bbox_of(&pts);
                    target.push(Feature {
                        class: class.clone(),
                        name: name.clone(),
                        pts,
                        bbox,
                    });
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::to_unit;

    const Z14: &[u8] = include_bytes!("../tests/bh_z14_6192_9115.pbf");
    const Z10: &[u8] = include_bytes!("../tests/bh_z10_387_569.pbf");
    const Z14_ID: TileId = TileId {
        z: 14,
        x: 6192,
        y: 9115,
    };
    const Z10_ID: TileId = TileId {
        z: 10,
        x: 387,
        y: 569,
    };

    fn bh_view(steps: f64) -> View {
        let mut v = View::world(1000.0, 700.0);
        v.center_on(-19.9167, -43.9345, steps);
        v
    }

    #[test]
    fn no_tiles_when_far_out() {
        assert_eq!(zoom_for_view(&View::world(1000.0, 700.0)), None);
        assert_eq!(zoom_for_view(&bh_view(DETAIL_MIN_STEPS - 0.5)), None);
    }

    #[test]
    fn zoom_is_deepest_that_fits() {
        for steps in [5.0, 7.0, 9.0, 11.0, 12.0] {
            let v = bh_view(steps);
            let z = zoom_for_view(&v).expect("zoom");
            assert!(count_in_view(&v, z) <= MAX_TILES_PER_VIEW, "steps {steps}");
            if z < MAX_ZOOM {
                assert!(
                    count_in_view(&v, z + 1) > MAX_TILES_PER_VIEW,
                    "steps {steps}"
                );
            }
        }
    }

    #[test]
    fn cover_is_nearest_first_and_contains_the_center() {
        let v = bh_view(9.0);
        let z = zoom_for_view(&v).unwrap();
        let tiles = cover(&v, z);
        assert_eq!(tiles.len(), count_in_view(&v, z));
        let (ux, uy) = to_unit(-19.9167, -43.9345);
        let n = (1u64 << z) as f64;
        let center = TileId {
            z,
            x: (ux * n).floor() as u32,
            y: (uy * n).floor() as u32,
        };
        assert_eq!(tiles[0], center);
    }

    #[test]
    fn cover_wraps_columns_and_stays_on_the_map() {
        let mut v = View::world(1000.0, 700.0);
        v.center_on(0.0, 179.9, 6.0);
        for t in cover(&v, 6) {
            assert!(t.x < 64 && t.y < 64, "{t:?}");
        }
    }

    #[test]
    fn decodes_z14_roads_names_and_places() {
        let d = decode(Z14_ID, Z14).expect("decode");
        assert!(d.roads.len() > 500, "roads {}", d.roads.len());
        assert!(d.roads.iter().any(|r| is_major_class(&r.class)));
        assert!(!d.road_names.is_empty());
        assert!(!d.places.is_empty(), "places");
        for f in d.roads.iter().chain(&d.road_names).chain(&d.places) {
            for p in &f.pts {
                assert!(
                    p[0] > 0.0 && p[0] < 1.0 && p[1] > 0.0 && p[1] < 1.0,
                    "{p:?}"
                );
            }
        }
    }

    #[test]
    fn decodes_z10_with_main_roads_only() {
        let d = decode(Z10_ID, Z10).expect("decode");
        assert!(!d.roads.is_empty());
        assert!(
            d.roads.iter().all(|r| is_major_class(&r.class)),
            "minor road at z10"
        );
    }

    #[test]
    fn display_uses_loaded_tiles_then_parents() {
        let v = bh_view(9.0);
        let z = zoom_for_view(&v).unwrap();
        let all = display_tiles(&v, z, |_| true);
        assert_eq!(all.len(), count_in_view(&v, z));
        assert!(all.iter().all(|t| t.z == z));
        assert!(display_tiles(&v, z, |_| false).is_empty());
        // Only the parents are loaded: each parent is listed once.
        let parents = display_tiles(&v, z, |t| t.z + 1 == z);
        assert!(!parents.is_empty() && parents.len() < all.len());
        let unique: HashSet<_> = parents.iter().collect();
        assert_eq!(unique.len(), parents.len());
        assert!(parents.iter().all(|t| t.z + 1 == z));
    }

    #[test]
    fn keep_set_has_covering_tiles_and_neighbours() {
        let v = bh_view(9.0);
        let z = zoom_for_view(&v).unwrap();
        let keep = keep_set(&v, z);
        for t in cover(&v, z) {
            assert!(keep.contains(&t));
            assert!(keep.contains(&parent(t)));
        }
        assert!(keep.len() > count_in_view(&v, z));
    }

    #[test]
    fn keep_set_wraps_columns_and_stays_on_the_map() {
        let mut v = View::world(1000.0, 700.0);
        v.center_on(0.0, 179.9, 6.0);
        for t in keep_set(&v, 6) {
            assert!(t.x < 64 && t.y < 64, "{t:?}");
        }
    }

    #[test]
    fn rejects_bytes_that_are_not_a_tile() {
        assert!(decode(Z14_ID, b"not a vector tile at all \xff\xff").is_err());
    }
}
