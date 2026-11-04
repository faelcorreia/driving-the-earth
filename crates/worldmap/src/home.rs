//! The home place, saved as `user://home.json` as `{"lat": ..., "lon": ...}` in degrees. Pure:
//! the caller reads and writes the file; this module parses, validates and formats the text.
use serde_json::{json, Value};

use crate::projection::MAX_LAT;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Home {
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HomeError {
    NotJson,
    NotAnObject,
    /// `lat` or `lon` missing or not a number.
    BadField(&'static str),
    /// Outside the map: latitude beyond +-85 or longitude beyond +-180, or not finite.
    OutOfRange,
}

impl Home {
    /// A home at this place; latitude must be within the map (+-85) and longitude within +-180.
    pub fn new(lat: f64, lon: f64) -> Result<Home, HomeError> {
        if !lat.is_finite() || !lon.is_finite() || lat.abs() > MAX_LAT || lon.abs() > 180.0 {
            return Err(HomeError::OutOfRange);
        }
        Ok(Home { lat, lon })
    }
}

/// Parses the text of a home file. Other keys (such as a street name) are ignored.
pub fn parse_home(text: &str) -> Result<Home, HomeError> {
    let value: Value = serde_json::from_str(text).map_err(|_| HomeError::NotJson)?;
    let obj = value.as_object().ok_or(HomeError::NotAnObject)?;
    let num = |key: &'static str| {
        obj.get(key)
            .and_then(Value::as_f64)
            .ok_or(HomeError::BadField(key))
    };
    let lat = num("lat")?;
    let lon = num("lon")?;
    Home::new(lat, lon)
}

/// Parses the old `"lat,lon"` text (environment variable `DTE_HOME`, degrees). Only used to import a
/// home saved before the home moved to `user://home.json`.
pub fn parse_pair(text: &str) -> Result<Home, HomeError> {
    let (lat, lon) = text.split_once(',').ok_or(HomeError::NotJson)?;
    let lat: f64 = lat.trim().parse().map_err(|_| HomeError::BadField("lat"))?;
    let lon: f64 = lon.trim().parse().map_err(|_| HomeError::BadField("lon"))?;
    Home::new(lat, lon)
}

/// What the game should use as home, decided from the texts it found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resolved {
    /// The home in use, `None` when the player has not chosen one.
    pub home: Option<Home>,
    /// Set only once: the home imported from the old sources, to be saved to `user://home.json`.
    pub import: Option<Home>,
}

/// The single rule for the home. `saved` is the text of `user://home.json` (`None` when the file
/// does not exist). Once that file exists it is the only source, even when it is malformed. Only
/// when it is missing, the old sources are imported once: `legacy_env` (`DTE_HOME`, `"lat,lon"`),
/// then `legacy_file` (`home.local.json`, the JSON with `lat` and `lon`).
pub fn resolve(
    saved: Option<&str>,
    legacy_env: Option<&str>,
    legacy_file: Option<&str>,
) -> Resolved {
    if let Some(text) = saved {
        return Resolved {
            home: parse_home(text).ok(),
            import: None,
        };
    }
    let imported = legacy_env
        .and_then(|t| parse_pair(t).ok())
        .or_else(|| legacy_file.and_then(|t| parse_home(t).ok()));
    Resolved {
        home: imported,
        import: imported,
    }
}

/// The text of a home file: pretty JSON with a trailing newline.
pub fn home_json(home: Home) -> String {
    let mut s = serde_json::to_string_pretty(&json!({ "lat": home.lat, "lon": home.lon }))
        .expect("numbers always format");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let home = Home::new(-19.9006792, -43.9174041).unwrap();
        let text = home_json(home);
        assert!(text.ends_with('\n'));
        assert_eq!(parse_home(&text), Ok(home));
    }

    #[test]
    fn ignores_extra_keys() {
        let h = parse_home(r#"{"lat": -19.9, "lon": -43.9, "street": "Rua X"}"#).unwrap();
        assert_eq!(
            h,
            Home {
                lat: -19.9,
                lon: -43.9
            }
        );
    }

    #[test]
    fn rejects_bad_files() {
        assert_eq!(parse_home("{nope"), Err(HomeError::NotJson));
        assert_eq!(parse_home("[1, 2]"), Err(HomeError::NotAnObject));
        assert_eq!(parse_home(r#"{"lon": 1}"#), Err(HomeError::BadField("lat")));
        assert_eq!(
            parse_home(r#"{"lat": "1", "lon": 1}"#),
            Err(HomeError::BadField("lat"))
        );
        assert_eq!(
            parse_home(r#"{"lat": 1, "lon": null}"#),
            Err(HomeError::BadField("lon"))
        );
        assert_eq!(
            parse_home(r#"{"lat": 86, "lon": 0}"#),
            Err(HomeError::OutOfRange)
        );
        assert_eq!(
            parse_home(r#"{"lat": 0, "lon": 200}"#),
            Err(HomeError::OutOfRange)
        );
    }

    #[test]
    fn limits_are_inclusive() {
        assert!(Home::new(85.0, 180.0).is_ok());
        assert!(Home::new(-85.0, -180.0).is_ok());
        assert_eq!(Home::new(f64::NAN, 0.0), Err(HomeError::OutOfRange));
        assert_eq!(Home::new(0.0, f64::INFINITY), Err(HomeError::OutOfRange));
    }

    #[test]
    fn saved_file_is_the_only_source_once_it_exists() {
        let legacy = Some("-19.9, -43.9");
        let saved = r#"{"lat": -20.0, "lon": -44.0}"#;
        let r = resolve(Some(saved), legacy, None);
        assert_eq!(r.import, None);
        assert_eq!(
            r.home,
            Some(Home {
                lat: -20.0,
                lon: -44.0
            })
        );
        // A malformed saved file is not replaced by the legacy sources.
        let r = resolve(Some("{nope"), legacy, Some(r#"{"lat": 1, "lon": 1}"#));
        assert_eq!((r.home, r.import), (None, None));
    }

    #[test]
    fn legacy_sources_are_imported_once_env_first() {
        let file = r#"{"lat": 1.5, "lon": -2.5, "street": "Rua X"}"#;
        let h = Home {
            lat: -3.25,
            lon: -4.75,
        };
        let r = resolve(None, Some(" -3.25 , -4.75 "), Some(file));
        assert_eq!((r.home, r.import), (Some(h), Some(h)));
        let only_file = Home {
            lat: 1.5,
            lon: -2.5,
        };
        let r = resolve(None, Some("  "), Some(file));
        assert_eq!((r.home, r.import), (Some(only_file), Some(only_file)));
        let r = resolve(None, None, None);
        assert_eq!((r.home, r.import), (None, None));
    }

    #[test]
    fn bad_legacy_values_are_not_imported() {
        assert!(parse_pair("1.5").is_err());
        assert!(parse_pair("95,0").is_err());
        assert_eq!(parse_pair("x,1"), Err(HomeError::BadField("lat")));
        assert_eq!(resolve(None, Some("95,0"), None).home, None);
    }
}
