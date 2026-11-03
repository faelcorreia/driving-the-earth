//! Areas, nature and street infrastructure from OSM (parks, water, waterways, trees, street
//! lamps, traffic signals, parking). Same blocks and same download mechanism as `osm`, stored in
//! `cache/<id>/raw/osm_areas/<bx>_<by>.json`. Read only here: this module does not render.
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;

use serde_json::Value;
use tileformat::MapMeta;

use crate::osm::{self, BlockStats, Downloader};

/// Directory of the layer under `raw/`.
pub const DIR_NAME: &str = "osm_areas";

const LEISURE: &str = "park|garden|pitch|playground|nature_reserve|recreation_ground";
const LANDUSE: &str = "grass|forest|meadow|recreation_ground|cemetery|residential|commercial|industrial|retail|farmland|village_green";
const NATURAL: &str = "wood|scrub|water|wetland|grassland|bare_rock|tree_row";
const WATERWAY: &str = "river|stream|canal|drain|ditch";
const SIGNALS: &str = "traffic_signals|stop|give_way|crossing";

/// Overpass query for the areas of the box `(south, west, north, east)`, with geometry.
/// Ways and relations (multipolygon members come with `out geom`) and the nodes listed below.
pub fn areas_query(bbox: (f64, f64, f64, f64)) -> String {
    let (s, w, n, e) = bbox;
    let bb = format!("({s},{w},{n},{e})");
    let clauses = [
        format!(r#"nwr["leisure"~"^({LEISURE})$"]{bb};"#),
        format!(r#"nwr["landuse"~"^({LANDUSE})$"]{bb};"#),
        format!(r#"nwr["natural"~"^({NATURAL})$"]{bb};"#),
        format!(r#"way["waterway"~"^({WATERWAY})$"]{bb};"#),
        format!(r#"way["water"]{bb};"#),
        format!(r#"relation["water"]{bb};"#),
        format!(r#"way["amenity"="parking"]{bb};"#),
        format!(r#"node["natural"="tree"]{bb};"#),
        format!(r#"node["highway"="street_lamp"]{bb};"#),
        format!(r#"node["highway"~"^({SIGNALS})$"]{bb};"#),
        format!(r#"node["man_made"="street_lamp"]{bb};"#),
    ];
    format!("[out:json][timeout:90];({});out body geom;", clauses.concat())
}

/// Checks an Overpass body before it is written. Overpass answers a timed-out query with
/// HTTP 200 and a `remark` such as "runtime error: Query timed out": that body is partial, so it is refused.
pub fn check_areas_body(v: &Value) -> io::Result<()> {
    osm::check_elements(v)?;
    if let Some(remark) = v["remark"].as_str() {
        if remark.contains("runtime error") {
            return Err(io::Error::new(io::ErrorKind::Other, format!("partial Overpass answer: {remark}")));
        }
    }
    Ok(())
}

/// Downloads the area blocks of `meta` that are not in `dir` yet (`osm_areas` directory).
pub fn download_missing_blocks(dir: &Path, meta: &MapMeta, dl: &Downloader) -> io::Result<BlockStats> {
    osm::download_blocks(dir, meta, dl, "areas", &areas_query, &check_areas_body)
}

/// Category of an element of the layer (first match wins). Anything else is `other`.
pub fn category(e: &Value) -> &'static str {
    let t = &e["tags"];
    let tag = |k: &str| t[k].as_str();
    if matches!(tag("leisure"), Some(v) if LEISURE.split('|').any(|x| x == v)) {
        return "park";
    }
    if e["type"].as_str() == Some("node") {
        return if tag("natural") == Some("tree") {
            "tree"
        } else if tag("highway") == Some("street_lamp") || tag("man_made") == Some("street_lamp") {
            "lamp"
        } else if matches!(tag("highway"), Some("traffic_signals" | "stop" | "give_way" | "crossing")) {
            "signal"
        } else {
            "other"
        };
    }
    if tag("natural") == Some("water") || t["water"].is_string() {
        "water"
    } else if tag("waterway").is_some() {
        "waterway"
    } else if tag("landuse").is_some() {
        "landuse"
    } else if tag("natural").is_some() {
        "natural"
    } else if tag("amenity") == Some("parking") {
        "parking"
    } else {
        "other"
    }
}

/// Calls `f` once for each element of the blocks of `meta`, in block order. Identity is
/// `(type, id)`: a node and a way may share a number. An element that crosses a block border is passed once.
pub fn visit_elements(dir: &Path, meta: &MapMeta, mut f: impl FnMut(&Value)) -> io::Result<()> {
    let mut seen: HashSet<(String, u64)> = HashSet::new();
    for (bx, by) in osm::blocks_for(meta) {
        let path = dir.join(osm::block_file_name(bx, by));
        let text = fs::read_to_string(&path).map_err(|e| {
            io::Error::new(e.kind(), format!("{}: {e} (download the area blocks first)", path.display()))
        })?;
        let v: Value = serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        let elements = v["elements"]
            .as_array()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, format!("{}: no 'elements'", path.display())))?;
        for e in elements {
            if let (Some(t), Some(id)) = (e["type"].as_str(), e["id"].as_u64()) {
                if !seen.insert((t.to_string(), id)) {
                    continue;
                }
            }
            f(e);
        }
    }
    Ok(())
}

/// Element counts of a layer: per category and per OSM type.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AreaCounts {
    pub by_category: BTreeMap<&'static str, usize>,
    /// Keys `node`, `way`, `relation`.
    pub by_type: BTreeMap<String, usize>,
}

/// Counts the elements of the layer in `dir`.
pub fn count_elements(dir: &Path, meta: &MapMeta) -> io::Result<AreaCounts> {
    let mut c = AreaCounts::default();
    visit_elements(dir, meta, |e| {
        *c.by_category.entry(category(e)).or_insert(0) += 1;
        let t = e["type"].as_str().unwrap_or("unknown").to_string();
        *c.by_type.entry(t).or_insert(0) += 1;
    })?;
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::time::Duration;

    use crate::osm::{HttpError, OverpassClient};

    fn meta(radius_m: f64) -> MapMeta {
        MapMeta {
            id: "t".into(),
            name: "t".into(),
            center_lat: -19.9,
            center_lon: -43.9,
            radius_m,
            tile_size_m: 250.0,
            created_unix: 0,
            complete: false,
        }
    }

    /// Scripted client: pops one answer per call and records the queries.
    struct FakeClient {
        answers: RefCell<Vec<Result<String, HttpError>>>,
        queries: RefCell<Vec<String>>,
    }

    impl FakeClient {
        fn new(mut answers: Vec<Result<String, HttpError>>) -> Self {
            answers.reverse();
            FakeClient { answers: RefCell::new(answers), queries: RefCell::new(Vec::new()) }
        }
    }

    impl OverpassClient for FakeClient {
        fn post(&self, query: &str) -> Result<String, HttpError> {
            self.queries.borrow_mut().push(query.to_string());
            self.answers
                .borrow_mut()
                .pop()
                .unwrap_or_else(|| Err(HttpError { status: None, message: "no more answers".into() }))
        }
    }

    fn http(status: u16) -> Result<String, HttpError> {
        Err(HttpError { status: Some(status), message: format!("HTTP {status}") })
    }

    fn downloader<'a>(client: &'a dyn OverpassClient, sleep: &'a dyn Fn(Duration)) -> Downloader<'a> {
        Downloader {
            client,
            sleep,
            min_gap: Duration::from_secs(1),
            max_attempts: 3,
            backoff_base: Duration::from_secs(5),
        }
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tilebake-areas-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// Body with one way of each family, as Overpass would answer for a block.
    fn body(ids: &[u64]) -> String {
        let els: Vec<String> = ids
            .iter()
            .map(|id| format!(r#"{{"type":"way","id":{id},"tags":{{"leisure":"park"}},"geometry":[{{"lat":-19.9,"lon":-43.9}}]}}"#))
            .collect();
        format!(r#"{{"elements":[{}]}}"#, els.join(","))
    }

    #[test]
    fn query_has_every_family_and_the_box() {
        let q = areas_query((-19.95, -43.95, -19.93, -43.92));
        assert!(q.starts_with("[out:json][timeout:90];("), "{q}");
        assert!(q.ends_with(");out body geom;"), "{q}");
        assert!(q.contains(r#"nwr["leisure"~"^(park|garden|pitch|playground|nature_reserve|recreation_ground)$"](-19.95,-43.95,-19.93,-43.92);"#));
        assert!(q.contains(r#"nwr["landuse"~"^(grass|"#) && q.contains("village_green)$\"]"));
        assert!(q.contains(r#"nwr["natural"~"^(wood|scrub|water|wetland|grassland|bare_rock|tree_row)$"]"#));
        assert!(q.contains(r#"way["waterway"~"^(river|stream|canal|drain|ditch)$"]"#));
        assert!(q.contains(r#"way["water"](-19.95"#) && q.contains(r#"relation["water"](-19.95"#));
        assert!(q.contains(r#"way["amenity"="parking"]"#));
        assert!(q.contains(r#"node["natural"="tree"]"#));
        assert!(q.contains(r#"node["highway"="street_lamp"]"#));
        assert!(q.contains(r#"node["highway"~"^(traffic_signals|stop|give_way|crossing)$"]"#));
        assert!(q.contains(r#"node["man_made"="street_lamp"]"#));
        // Waterways and parking are ways only; no node query for them.
        assert!(!q.contains(r#"nwr["waterway"]"#) && !q.contains(r#"node["waterway"]"#));
        assert!(!q.contains(r#"node["amenity"]"#));
    }

    #[test]
    fn block_file_names_match_osm_grid() {
        assert_eq!(osm::block_file_name(-1, 3), "-1_3.json");
        assert_eq!(DIR_NAME, "osm_areas");
        // Same grid as the OSM layer: one km radius is four blocks, ten km is one hundred.
        assert_eq!(osm::blocks_for(&meta(1000.0)).len(), 4);
        assert_eq!(osm::blocks_for(&meta(10000.0)).len(), 100);
    }

    #[test]
    fn retry_on_429_and_504_counts_the_retries() {
        let client = FakeClient::new(vec![http(429), http(504), Ok(body(&[1]))]);
        let waits = RefCell::new(Vec::new());
        let sleep = |d: Duration| waits.borrow_mut().push(d);
        let mut dl = downloader(&client, &sleep);
        dl.max_attempts = 5;
        let (text, retries) = dl.post_with_retry_counted("q").unwrap();
        assert_eq!(text, body(&[1]));
        assert_eq!(retries, 2);
        assert_eq!(*waits.borrow(), vec![Duration::from_secs(5), Duration::from_secs(10)]);
    }

    #[test]
    fn download_writes_blocks_counts_retries_and_is_idempotent() {
        let dir = temp_dir("download");
        let m = meta(1000.0);
        let client = FakeClient::new(vec![http(503), Ok(body(&[1])), Ok(body(&[2])), Ok(body(&[3])), Ok(body(&[4]))]);
        let sleep = |_: Duration| {};
        let stats = download_missing_blocks(&dir, &m, &downloader(&client, &sleep)).unwrap();
        assert_eq!(stats.blocks, 4);
        assert_eq!(stats.downloaded, 4);
        assert_eq!(stats.retries, 1);
        assert_eq!(client.queries.borrow().len(), 5);
        assert!(stats.bytes_written > 0);
        assert!(client.queries.borrow()[0].contains("nwr[\"leisure\""));
        assert!(dir.join("-1_-1.json").exists());
        assert!(!dir.join("-1_-1.json.part").exists());
        // Second run: nothing is queried.
        let client2 = FakeClient::new(vec![]);
        let again = download_missing_blocks(&dir, &m, &downloader(&client2, &sleep)).unwrap();
        assert_eq!((again.downloaded, again.retries), (0, 0));
        assert!(client2.queries.borrow().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn partial_answer_is_not_written() {
        let dir = temp_dir("partial");
        let m = meta(1000.0);
        let partial = r#"{"elements":[],"remark":"runtime error: Query timed out in \"query\" at line 1"}"#;
        let client = FakeClient::new(vec![Ok(partial.into())]);
        let sleep = |_: Duration| {};
        let err = download_missing_blocks(&dir, &m, &downloader(&client, &sleep)).unwrap_err();
        assert!(err.to_string().contains("partial Overpass answer"), "{err}");
        assert!(!dir.join("-1_-1.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn categories_of_nodes_and_ways() {
        let node = |tags: &str| serde_json::from_str::<Value>(&format!(r#"{{"type":"node","id":1,"tags":{{{tags}}}}}"#)).unwrap();
        let way = |tags: &str| serde_json::from_str::<Value>(&format!(r#"{{"type":"way","id":1,"tags":{{{tags}}}}}"#)).unwrap();
        assert_eq!(category(&node(r#""natural":"tree""#)), "tree");
        assert_eq!(category(&node(r#""highway":"street_lamp""#)), "lamp");
        assert_eq!(category(&node(r#""man_made":"street_lamp""#)), "lamp");
        assert_eq!(category(&node(r#""leisure":"playground""#)), "park");
        assert_eq!(category(&node(r#""highway":"crossing""#)), "signal");
        assert_eq!(category(&node(r#""highway":"give_way""#)), "signal");
        assert_eq!(category(&way(r#""leisure":"pitch""#)), "park");
        assert_eq!(category(&way(r#""natural":"water""#)), "water");
        assert_eq!(category(&way(r#""water":"lake""#)), "water");
        assert_eq!(category(&way(r#""waterway":"river","name":"Ribeirão Arrudas""#)), "waterway");
        assert_eq!(category(&way(r#""landuse":"residential""#)), "landuse");
        assert_eq!(category(&way(r#""natural":"wood""#)), "natural");
        assert_eq!(category(&way(r#""amenity":"parking""#)), "parking");
        assert_eq!(category(&way(r#""building":"yes""#)), "other");
    }

    #[test]
    fn counts_by_category_and_type_and_dedupes_by_type_and_id() {
        let dir = temp_dir("counts");
        let m = meta(1000.0);
        fs::create_dir_all(&dir).unwrap();
        for (bx, by) in osm::blocks_for(&m) {
            // Node 5 and way 5 are different elements; the way 9 crosses the border (in two blocks).
            let text = if (bx, by) == (-1, -1) {
                r#"{"elements":[{"type":"node","id":5,"tags":{"natural":"tree"}},{"type":"way","id":5,"tags":{"leisure":"park"}},{"type":"way","id":9,"tags":{"waterway":"river"}}]}"#
            } else if (bx, by) == (0, -1) {
                r#"{"elements":[{"type":"way","id":9,"tags":{"waterway":"river"}},{"type":"relation","id":3,"tags":{"natural":"water"}}]}"#
            } else {
                r#"{"elements":[]}"#
            };
            fs::write(dir.join(osm::block_file_name(bx, by)), text).unwrap();
        }
        let c = count_elements(&dir, &m).unwrap();
        assert_eq!(c.by_category.get("tree"), Some(&1));
        assert_eq!(c.by_category.get("park"), Some(&1));
        assert_eq!(c.by_category.get("waterway"), Some(&1));
        assert_eq!(c.by_category.get("water"), Some(&1));
        assert_eq!(c.by_type.get("node"), Some(&1));
        assert_eq!(c.by_type.get("way"), Some(&2));
        assert_eq!(c.by_type.get("relation"), Some(&1));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn visit_reports_missing_block() {
        let dir = temp_dir("missing");
        fs::create_dir_all(&dir).unwrap();
        let err = visit_elements(&dir, &meta(1000.0), |_| {}).unwrap_err();
        assert!(err.to_string().contains("download the area blocks first"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn body_check_needs_elements_and_no_runtime_error() {
        let ok: Value = serde_json::from_str(r#"{"elements":[]}"#).unwrap();
        assert!(check_areas_body(&ok).is_ok());
        let no_elements: Value = serde_json::from_str(r#"{"remark":"x"}"#).unwrap();
        assert!(check_areas_body(&no_elements).is_err());
    }
}
