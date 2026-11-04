//! Web Mercator on the unit square: x grows east from lon -180 (0) to 180 (1); y is 0 at the north
//! limit and 1 at the south limit. Latitude is clamped to +-85 degrees, where Mercator is finite.
use std::f64::consts::PI;

/// Latitude limit of the map, in degrees.
pub const MAX_LAT: f64 = 85.0;

/// Clamps a latitude to the map's limit.
pub fn clamp_lat(lat: f64) -> f64 {
    lat.clamp(-MAX_LAT, MAX_LAT)
}

/// Degrees to unit square. Longitude is not wrapped: 181 degrees gives an x just above 1.
pub fn to_unit(lat: f64, lon: f64) -> (f64, f64) {
    let s = clamp_lat(lat).to_radians().sin();
    let x = (lon + 180.0) / 360.0;
    let y = 0.5 - ((1.0 + s) / (1.0 - s)).ln() / (4.0 * PI);
    (x, y)
}

/// Unit square to `(lat, lon)` in degrees. The longitude is wrapped to [-180, 180).
pub fn from_unit(x: f64, y: f64) -> (f64, f64) {
    let n = PI * (1.0 - 2.0 * y);
    let lat = clamp_lat(n.tanh().asin().to_degrees());
    let lon = (x * 360.0 - 180.0 + 180.0).rem_euclid(360.0) - 180.0;
    (lat, lon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_and_corners() {
        let (x, y) = to_unit(0.0, 0.0);
        assert!((x - 0.5).abs() < 1e-12 && (y - 0.5).abs() < 1e-12);
        let (x, y) = to_unit(MAX_LAT, -180.0);
        assert!(
            x.abs() < 1e-12 && y < 0.002,
            "north west corner at ({x}, {y})"
        );
    }

    #[test]
    fn round_trip() {
        for &(lat, lon) in &[(-19.9, -43.9), (51.5, -0.1), (-84.0, 179.0), (0.0, -180.0)] {
            let (x, y) = to_unit(lat, lon);
            let (lat2, lon2) = from_unit(x, y);
            assert!((lat - lat2).abs() < 1e-9, "lat {lat} -> {lat2}");
            assert!((lon - lon2).abs() < 1e-9, "lon {lon} -> {lon2}");
        }
    }

    #[test]
    fn latitude_is_clamped() {
        assert_eq!(clamp_lat(90.0), MAX_LAT);
        assert_eq!(clamp_lat(-90.0), -MAX_LAT);
        assert_eq!(to_unit(90.0, 0.0), to_unit(MAX_LAT, 0.0));
        assert!(to_unit(89.9, 0.0).1.is_finite());
    }

    #[test]
    fn longitude_wraps_back() {
        let (_, lon) = from_unit(to_unit(0.0, 190.0).0, 0.5);
        assert!((lon - (-170.0)).abs() < 1e-9);
    }
}
