//! Pipeline OSM + DEM -> tiles. Chamado pelo GDExtension numa thread própria.
use serde_json::Value;
use std::fs;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tileformat::MapMeta;

const USER_AGENT: &str = "driving-the-earth/0.1 (tilebake; projeto pessoal)";
const NOMINATIM_URL: &str = "https://nominatim.openstreetmap.org/search";
const OVERPASS_URL: &str = "https://overpass-api.de/api/interpreter";

/// Raio de coleta ao redor da casa, em metros.
pub const RAIO_PADRAO_M: f64 = 1000.0;
/// Lado do tile da grade, em metros.
pub const TILE_PADRAO_M: f64 = 250.0;

/// Resultado de `baixar_osm`: metadados e contagens do bruto.
#[derive(Debug)]
pub struct Resumo {
    pub meta: MapMeta,
    /// Ways com tag `highway`.
    pub vias: usize,
    /// Ways com tag `building`.
    pub predios: usize,
    /// Geocodificação veio de `meta.json` já existente.
    pub geocodificacao_em_cache: bool,
    /// Bruto do Overpass veio de `raw/overpass.json` já existente.
    pub osm_em_cache: bool,
}

fn erro<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}

/// Geocodifica o endereço (Nominatim) e devolve (lat, lon, resposta bruta).
fn geocodificar(endereco: &str) -> io::Result<(f64, f64, String)> {
    let texto = ureq::get(NOMINATIM_URL)
        .set("User-Agent", USER_AGENT)
        .query("q", endereco)
        .query("format", "json")
        .query("limit", "1")
        .call()
        .map_err(erro)?
        .into_string()?;
    let v: Value = serde_json::from_str(&texto).map_err(erro)?;
    let primeiro = v
        .get(0)
        .ok_or_else(|| erro("Nominatim não encontrou o endereço"))?;
    let lat: f64 = primeiro["lat"]
        .as_str()
        .ok_or_else(|| erro("Nominatim sem 'lat'"))?
        .parse()
        .map_err(erro)?;
    let lon: f64 = primeiro["lon"]
        .as_str()
        .ok_or_else(|| erro("Nominatim sem 'lon'"))?
        .parse()
        .map_err(erro)?;
    Ok((lat, lon, texto))
}

/// Conta (vias, prédios) numa resposta bruta do Overpass.
fn contar(overpass_json: &str) -> io::Result<(usize, usize)> {
    let v: Value = serde_json::from_str(overpass_json).map_err(erro)?;
    let elementos = v["elements"]
        .as_array()
        .ok_or_else(|| erro("resposta do Overpass sem 'elements'"))?;
    let vias = elementos.iter().filter(|e| e["tags"]["highway"].is_string()).count();
    let predios = elementos.iter().filter(|e| e["tags"]["building"].is_string()).count();
    Ok((vias, predios))
}

/// Baixa o OSM do raio ao redor do endereço e grava o bruto em `cache_root/<id>/raw/`.
///
/// Reaproveita o cache: se `meta.json` existe, não geocodifica de novo; se
/// `raw/overpass.json` existe, não consulta o Overpass de novo.
pub fn baixar_osm(cache_root: &Path, id: &str, nome: &str, endereco: &str) -> io::Result<Resumo> {
    let dir = cache_root.join(id);
    let raw = dir.join("raw");
    fs::create_dir_all(&raw)?;
    let meta_path = dir.join("meta.json");
    let nominatim_path = raw.join("nominatim.json");
    let overpass_path = raw.join("overpass.json");

    let (meta, geocodificacao_em_cache) = if meta_path.exists() {
        let file = fs::File::open(&meta_path)?;
        let meta: MapMeta = serde_json::from_reader(io::BufReader::new(file)).map_err(erro)?;
        (meta, true)
    } else {
        let (lat, lon, bruto) = geocodificar(endereco)?;
        fs::write(&nominatim_path, bruto)?;
        let created_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(erro)?
            .as_secs();
        let meta = MapMeta {
            id: id.to_string(),
            name: nome.to_string(),
            center_lat: lat,
            center_lon: lon,
            radius_m: RAIO_PADRAO_M,
            tile_size_m: TILE_PADRAO_M,
            created_unix,
            complete: false,
        };
        fs::write(&meta_path, serde_json::to_string_pretty(&meta).map_err(erro)?)?;
        (meta, false)
    };

    let (osm_json, osm_em_cache) = if overpass_path.exists() {
        (fs::read_to_string(&overpass_path)?, true)
    } else {
        let query = format!(
            "[out:json][timeout:90];(way[\"highway\"](around:{r},{lat},{lon});way[\"building\"](around:{r},{lat},{lon}););out body geom;",
            r = meta.radius_m,
            lat = meta.center_lat,
            lon = meta.center_lon,
        );
        let corpo = ureq::post(OVERPASS_URL)
            .set("User-Agent", USER_AGENT)
            .send_form(&[("data", query.as_str())])
            .map_err(erro)?
            .into_string()?;
        contar(&corpo)?; // valida antes de gravar
        fs::write(&overpass_path, &corpo)?;
        (corpo, false)
    };

    let (vias, predios) = contar(&osm_json)?;
    Ok(Resumo {
        meta,
        vias,
        predios,
        geocodificacao_em_cache,
        osm_em_cache,
    })
}
