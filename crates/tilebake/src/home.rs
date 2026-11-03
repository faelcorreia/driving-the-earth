//! Home position: the reference point of the sky (sun and moon) and of the studies. This is the
//! only place that reads it. Precedence: the `DTE_HOME` environment variable (`"lat,lon"`, degrees),
//! then `home.local.json` at the repository root (not versioned), then a generic public default.
//! A malformed value panics: a wrong home would silently move the sun.
use serde::Deserialize;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::OnceLock;

/// Environment variable with the home, `"lat,lon"` in degrees.
pub const HOME_ENV: &str = "DTE_HOME";
/// Local file with the home, at the repository root: `{"lat": .., "lon": .., "street": ".."}`.
pub const HOME_FILE: &str = "home.local.json";
/// Generic public default, used when neither `DTE_HOME` nor the local file is set: Praça Sete,
/// Belo Horizonte, degrees.
pub const DEFAULT_LAT_DEG: f64 = -19.9191;
pub const DEFAULT_LON_DEG: f64 = -43.9386;
/// Largest absolute latitude, degrees.
const MAX_LAT_DEG: f64 = 90.0;
/// Largest absolute longitude, degrees.
const MAX_LON_DEG: f64 = 180.0;

/// Where the home was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Env,
    File,
    Default,
}

/// The home: position in degrees and, when the local file gives one, the street name.
#[derive(Debug, Clone, PartialEq)]
pub struct Home {
    /// Latitude, degrees (south negative).
    pub lat_deg: f64,
    /// Longitude, degrees (east positive).
    pub lon_deg: f64,
    /// Street name, from the local file only.
    pub street: Option<String>,
    pub source: Source,
}

#[derive(Deserialize)]
struct FileHome {
    lat: f64,
    lon: f64,
    street: Option<String>,
}

static HOME: OnceLock<Home> = OnceLock::new();

/// The home of this build, read once per process from the environment and the repository root.
pub fn home() -> &'static Home {
    HOME.get_or_init(|| {
        let file_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(HOME_FILE);
        let file = match fs::read_to_string(&file_path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => panic!("cannot read {}: {e}", file_path.display()),
        };
        resolve(std::env::var(HOME_ENV).ok().as_deref(), file.as_deref())
    })
}

/// Pure resolution from the raw values: `env` is `DTE_HOME`, `file` is the content of the local file.
pub fn resolve(env: Option<&str>, file: Option<&str>) -> Home {
    if let Some(text) = env.filter(|t| !t.trim().is_empty()) {
        let (lat_deg, lon_deg) = parse_env(text).unwrap_or_else(|e| panic!("{HOME_ENV}: {e}"));
        return Home {
            lat_deg,
            lon_deg,
            street: None,
            source: Source::Env,
        };
    }
    if let Some(text) = file {
        return parse_file(text).unwrap_or_else(|e| panic!("{HOME_FILE}: {e}"));
    }
    Home {
        lat_deg: DEFAULT_LAT_DEG,
        lon_deg: DEFAULT_LON_DEG,
        street: None,
        source: Source::Default,
    }
}

/// Parses `"lat,lon"` in degrees.
pub fn parse_env(text: &str) -> Result<(f64, f64), String> {
    let (lat, lon) = text
        .split_once(',')
        .ok_or("expected \"lat,lon\" in degrees")?;
    let lat: f64 = lat.trim().parse().map_err(|e| format!("latitude: {e}"))?;
    let lon: f64 = lon.trim().parse().map_err(|e| format!("longitude: {e}"))?;
    check_range(lat, lon)
}

/// Parses the local JSON file.
pub fn parse_file(text: &str) -> Result<Home, String> {
    let f: FileHome = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let (lat_deg, lon_deg) = check_range(f.lat, f.lon)?;
    Ok(Home {
        lat_deg,
        lon_deg,
        street: f.street,
        source: Source::File,
    })
}

fn check_range(lat: f64, lon: f64) -> Result<(f64, f64), String> {
    if !lat.is_finite() || lat.abs() > MAX_LAT_DEG {
        return Err(format!("latitude {lat} out of range"));
    }
    if !lon.is_finite() || lon.abs() > MAX_LON_DEG {
        return Err(format!("longitude {lon} out of range"));
    }
    Ok((lat, lon))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_when_nothing_is_set() {
        let h = resolve(None, None);
        assert_eq!(h.source, Source::Default);
        assert_eq!((h.lat_deg, h.lon_deg), (DEFAULT_LAT_DEG, DEFAULT_LON_DEG));
        assert_eq!(h.street, None);
    }

    #[test]
    fn env_wins_over_file_and_blank_env_is_ignored() {
        let file = r#"{"lat": 1.5, "lon": -2.5, "street": "Rua Exemplo"}"#;
        let h = resolve(Some("-3.25, -4.75"), Some(file));
        assert_eq!(
            (h.source, h.lat_deg, h.lon_deg),
            (Source::Env, -3.25, -4.75)
        );
        assert_eq!(h.street, None);
        let h = resolve(Some("  "), Some(file));
        assert_eq!((h.source, h.lat_deg, h.lon_deg), (Source::File, 1.5, -2.5));
    }

    #[test]
    fn file_gives_position_and_optional_street() {
        let h = parse_file(r#"{"lat": 1.5, "lon": -2.5, "street": "Rua Exemplo"}"#).unwrap();
        assert_eq!(h.street.as_deref(), Some("Rua Exemplo"));
        let h = parse_file(r#"{"lat": 1.5, "lon": -2.5}"#).unwrap();
        assert_eq!((h.lat_deg, h.lon_deg, h.street), (1.5, -2.5, None));
    }

    #[test]
    fn malformed_values_are_errors() {
        assert!(parse_env("1.5").is_err());
        assert!(parse_env("x,1").is_err());
        assert!(parse_env("95,0").is_err());
        assert!(parse_env("0,181").is_err());
        assert!(parse_file(r#"{"lat": 1.5}"#).is_err());
        assert!(parse_file(r#"{"lat": 1.5, "lon": 200}"#).is_err());
    }
}
