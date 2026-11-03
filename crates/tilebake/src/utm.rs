//! Transverse Mercator (UTM, Krüger series, GRS80). Forward only: the bake needs lat/lon -> UTM.
const A: f64 = 6_378_137.0;
const F: f64 = 1.0 / 298.257_222_101;
/// Central meridian of UTM zone 23S (SIRGAS2000, EPSG:31983).
pub const UTM23S_LON0: f64 = -45.0;
const K0: f64 = 0.9996;
const FALSE_EAST: f64 = 500_000.0;
const FALSE_NORTH: f64 = 10_000_000.0;

/// `(easting, northing)` in meters of `(lat, lon)` in degrees, for the zone centered at `lon0_deg`.
pub fn forward(lat_deg: f64, lon_deg: f64, lon0_deg: f64) -> (f64, f64) {
    let n = F / (2.0 - F);
    let e = (F * (2.0 - F)).sqrt();
    let a_rad = A / (1.0 + n) * (1.0 + n * n / 4.0 + n.powi(4) / 64.0);
    let alphas = [
        n / 2.0 - 2.0 * n * n / 3.0 + 5.0 * n.powi(3) / 16.0,
        13.0 * n * n / 48.0 - 3.0 * n.powi(3) / 5.0,
        61.0 * n.powi(3) / 240.0,
    ];
    let tau = lat_deg.to_radians().tan();
    let lam = (lon_deg - lon0_deg).to_radians();
    let sigma = (e * (e * tau / (1.0 + tau * tau).sqrt()).atanh()).sinh();
    let tau_p = tau * (1.0 + sigma * sigma).sqrt() - sigma * (1.0 + tau * tau).sqrt();
    let xi_p = tau_p.atan2(lam.cos());
    let eta_p = (lam.sin() / (tau_p * tau_p + lam.cos() * lam.cos()).sqrt()).atanh();
    let (mut xi, mut eta) = (xi_p, eta_p);
    for (k, al) in alphas.iter().enumerate() {
        let j = (k + 1) as f64 * 2.0;
        xi += al * (j * xi_p).sin() * (j * eta_p).cosh();
        eta += al * (j * xi_p).cos() * (j * eta_p).sinh();
    }
    (FALSE_EAST + K0 * a_rad * eta, FALSE_NORTH + K0 * a_rad * xi)
}

/// UTM 23S `(easting, northing)` of `(lat, lon)`.
pub fn utm23s(lat_deg: f64, lon_deg: f64) -> (f64, f64) {
    forward(lat_deg, lon_deg, UTM23S_LON0)
}

/// Central meridian (degrees) of the SIRGAS 2000 UTM zone of a package `crs` string.
/// Accepts `EPSG:31978` (zone 18S) to `EPSG:31985` (zone 25S). Other CRS are rejected: the
/// projection is only implemented for these zones.
pub fn sirgas_zone_lon0(crs: &str) -> Result<f64, String> {
    let code: u32 = crs
        .trim()
        .strip_prefix("EPSG:")
        .and_then(|c| c.trim().parse().ok())
        .ok_or_else(|| format!("unsupported crs '{crs}': expected 'EPSG:<code>'"))?;
    if !(31978..=31985).contains(&code) {
        return Err(format!("unsupported crs '{crs}': only SIRGAS 2000 UTM 18S-25S (EPSG:31978-31985)"));
    }
    let zone = (code - 31960) as f64;
    Ok(-183.0 + 6.0 * zone)
}

/// Inverse of `forward`: `(lat, lon)` in degrees of the UTM point `(easting, northing)` for the zone
/// centered at `lon0_deg`. Newton iteration on `forward` with a finite-difference Jacobian.
pub fn inverse(e: f64, n: f64, lon0_deg: f64) -> (f64, f64) {
    // First guess: 1 degree of latitude is ~110.6 km (southern hemisphere, northing below the false north).
    let mut lat = (n - FALSE_NORTH) / (K0 * 110_600.0);
    let mut lon = lon0_deg + (e - FALSE_EAST) / (K0 * 111_000.0 * lat.to_radians().cos());
    let h = 1e-7;
    for _ in 0..50 {
        let (fe, fn_) = forward(lat, lon, lon0_deg);
        let (re, rn) = (fe - e, fn_ - n);
        if re.abs() < 1e-7 && rn.abs() < 1e-7 {
            break;
        }
        let (ea, na) = forward(lat + h, lon, lon0_deg);
        let (eb, nb) = forward(lat, lon + h, lon0_deg);
        let (a, b, c, d) = ((ea - fe) / h, (eb - fe) / h, (na - fn_) / h, (nb - fn_) / h);
        let det = a * d - b * c;
        let dlat = (d * re - b * rn) / det;
        let dlon = (a * rn - c * re) / det;
        lat -= dlat;
        lon -= dlon;
    }
    (lat, lon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn central_meridian_has_false_easting() {
        let (e, n) = utm23s(-19.9, UTM23S_LON0);
        assert!((e - FALSE_EAST).abs() < 1e-6, "{e}");
        // Meridian arc from the equator to lat -19.9 is ~2 201 270 m; times k0 = 0.9996.
        assert!((n - 7_799_600.0).abs() < 100.0, "{n}");
    }

    #[test]
    fn longitude_step_matches_local_scale() {
        // Ellipsoid: N(lat) * cos(lat) * 0.01 deg, times k0 = 0.9996. The scale grows with the distance to
        // the central meridian (110 km here), so allow 0.1%: the spherical MapGrid would be off by 1.8 m.
        let (e0, _) = utm23s(-19.9, -43.9);
        let (e1, _) = utm23s(-19.9, -43.89);
        let phi = (-19.9f64).to_radians();
        let e2 = F * (2.0 - F);
        let n_rad = A / (1.0 - e2 * phi.sin().powi(2)).sqrt();
        let expected = n_rad * phi.cos() * 0.01f64.to_radians() * K0;
        assert!(((e1 - e0) - expected).abs() < 1.0, "{} vs {expected}", e1 - e0);
    }

    #[test]
    fn inverse_round_trips_forward() {
        for (lat, lon) in [(-19.9, -43.9), (-19.5, -43.2), (-20.3, -44.5)] {
            let (e, n) = utm23s(lat, lon);
            let (lat2, lon2) = inverse(e, n, UTM23S_LON0);
            assert!((lat2 - lat).abs() < 1e-8 && (lon2 - lon).abs() < 1e-8, "{lat2} {lon2} vs {lat} {lon}");
        }
    }

    #[test]
    fn sirgas_zone_from_epsg() {
        assert!((sirgas_zone_lon0("EPSG:31983").unwrap() - UTM23S_LON0).abs() < 1e-12);
        assert!((sirgas_zone_lon0("EPSG:31978").unwrap() + 75.0).abs() < 1e-12);
        assert!(sirgas_zone_lon0("EPSG:4326").is_err());
        assert!(sirgas_zone_lon0("EPSG:31983 (SIRGAS 2000 / UTM zone 23S)").is_err());
        assert!(sirgas_zone_lon0("UTM").is_err());
    }
}
