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
}
