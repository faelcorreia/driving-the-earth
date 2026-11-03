//! Astronomia e luz do ambiente, pura (sem Godot): posição do sol e da lua, fase, luz global,
//! limiares com histerese e atraso por id. Ângulos em graus, tempo em segundos Unix (UTC),
//! distâncias em metros. Fórmulas de baixa precisão (Meeus, cap. 25 e 47), suficientes para
//! uma cena: erro do sol de ~0,01 grau e da lua de ~0,3 grau.

use std::f64::consts::{PI, TAU};

/// Latitude da casa (Rua São Mateus, 341, Sagrada Família, Belo Horizonte), graus.
pub const HOME_LAT_DEG: f64 = -19.9006792;
/// Longitude da casa, graus (leste positivo).
pub const HOME_LON_DEG: f64 = -43.9174041;
/// Fuso de Belo Horizonte, UTC-3 (sem horário de verão desde 2019), em segundos.
pub const LOCAL_UTC_OFFSET_S: f64 = -3.0 * 3600.0;
/// Altitude do centro do sol no nascer/pôr aparente (refração + semidiâmetro), graus.
pub const SUN_RISE_ALT_DEG: f64 = -0.833;
/// Altitude do sol em que a luz solar chega a zero (crepúsculo civil), graus.
pub const SUN_DARK_DEG: f64 = -6.0;
/// Altitude do sol em que a luz solar chega ao máximo, graus.
pub const SUN_FULL_DEG: f64 = 6.0;
/// Contribuição máxima da lua à luz global (lua cheia e alta), fração de 0..1.
pub const MOON_MAX_LIGHT: f64 = 0.08;
/// Altura da lua em que a contribuição lunar é plena, graus.
pub const MOON_FULL_ALT_DEG: f64 = 20.0;

const SECONDS_PER_DAY: f64 = 86400.0;
/// Dia Juliano do instante Unix 0.
const JD_UNIX_EPOCH: f64 = 2440587.5;
/// Dia Juliano de J2000.0.
const JD_J2000: f64 = 2451545.0;
/// Obliquidade da eclíptica em J2000, graus.
const OBLIQUITY_J2000_DEG: f64 = 23.439291;

/// Cor linear RGB, cada canal em 0..1.
pub type Rgb = [f32; 3];

/// Posição de um astro no céu do observador: azimute a partir do norte, sentido horário, e altura.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Horizontal {
    pub azimuth_deg: f64,
    pub altitude_deg: f64,
}

/// Estado da lua: posição, fração iluminada (0 nova, 1 cheia) e se está crescendo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoonState {
    pub position: Horizontal,
    pub illumination: f64,
    pub waxing: bool,
}

/// Dia Juliano de um instante Unix (segundos UTC).
pub fn julian_day(unix_s: f64) -> f64 {
    unix_s / SECONDS_PER_DAY + JD_UNIX_EPOCH
}

/// Instante Unix (UTC, segundos) de uma data e hora local de Belo Horizonte.
/// `hour` é decimal: 17.5 = 17:30.
pub fn local_to_unix(year: i64, month: u32, day: u32, hour: f64) -> f64 {
    let days = days_from_civil(year, month, day) as f64;
    days * SECONDS_PER_DAY + hour * 3600.0 - LOCAL_UTC_OFFSET_S
}

/// Hora local decimal (0..24) de um instante Unix em Belo Horizonte.
pub fn local_hour(unix_s: f64) -> f64 {
    (unix_s + LOCAL_UTC_OFFSET_S).rem_euclid(SECONDS_PER_DAY) / 3600.0
}

/// Dias desde 1970-01-01 de uma data do calendário gregoriano (algoritmo de H. Hinnant).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = month as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Obliquidade da eclíptica no tempo juliano `jd`, em radianos.
fn obliquity_rad(jd: f64) -> f64 {
    let t = (jd - JD_J2000) / 36525.0;
    (OBLIQUITY_J2000_DEG - 0.0130042 * t).to_radians()
}

/// Longitude eclíptica aparente do sol no tempo juliano `jd`, radianos.
fn sun_ecliptic_lon_rad(jd: f64) -> f64 {
    let t = (jd - JD_J2000) / 36525.0;
    let l0 = 280.46646 + 36000.76983 * t;
    let m = (357.52911 + 35999.05029 * t).to_radians();
    let c = (1.914602 - 0.004817 * t) * m.sin()
        + 0.019993 * (2.0 * m).sin()
        + 0.000289 * (3.0 * m).sin();
    let omega = (125.04 - 1934.136 * t).to_radians();
    (l0 + c - 0.00569 - 0.00478 * omega.sin()).to_radians()
}

/// Converte ascensão reta e declinação (radianos) em azimute/altura para a casa.
fn horizontal(jd: f64, ra_rad: f64, dec_rad: f64) -> Horizontal {
    let lat = HOME_LAT_DEG.to_radians();
    let gmst_deg = (280.46061837 + 360.98564736629 * (jd - JD_J2000)).rem_euclid(360.0);
    let ha = (gmst_deg + HOME_LON_DEG - ra_rad.to_degrees()).to_radians();
    let sin_alt = lat.sin() * dec_rad.sin() + lat.cos() * dec_rad.cos() * ha.cos();
    // Azimute medido a partir do sul, para oeste; convertido para norte, sentido horário.
    let az_from_south = ha
        .sin()
        .atan2(ha.cos() * lat.sin() - dec_rad.tan() * lat.cos());
    Horizontal {
        azimuth_deg: (az_from_south + PI).rem_euclid(TAU).to_degrees(),
        altitude_deg: sin_alt.clamp(-1.0, 1.0).asin().to_degrees(),
    }
}

/// Posição do sol no instante Unix `unix_s`, vista de Belo Horizonte.
pub fn sun(unix_s: f64) -> Horizontal {
    let jd = julian_day(unix_s);
    let lon = sun_ecliptic_lon_rad(jd);
    let eps = obliquity_rad(jd);
    let ra = (eps.cos() * lon.sin()).atan2(lon.cos());
    let dec = (eps.sin() * lon.sin()).asin();
    horizontal(jd, ra, dec)
}

/// Posição e fase da lua no instante Unix `unix_s`.
pub fn moon(unix_s: f64) -> MoonState {
    let jd = julian_day(unix_s);
    let d = jd - JD_J2000;
    let lm = (218.316 + 13.176396 * d).to_radians();
    let mm = (134.963 + 13.064993 * d).to_radians();
    let f = (93.272 + 13.229350 * d).to_radians();
    let lon = lm + (6.289_f64).to_radians() * mm.sin();
    let lat = (5.128_f64).to_radians() * f.sin();
    let eps = obliquity_rad(jd);
    let ra = (lon.sin() * eps.cos() - lat.tan() * eps.sin()).atan2(lon.cos());
    let dec = (lat.sin() * eps.cos() + lat.cos() * eps.sin() * lon.sin()).asin();
    // Elongação geocêntrica lua-sol: cos(D) = cos(lat) cos(lon - lon_sol).
    let dlon = lon - sun_ecliptic_lon_rad(jd);
    let elong = (lat.cos() * dlon.cos()).clamp(-1.0, 1.0).acos();
    MoonState {
        position: horizontal(jd, ra, dec),
        illumination: (1.0 - elong.cos()) / 2.0,
        waxing: dlon.rem_euclid(TAU) < PI,
    }
}

/// Primeiro instante, a partir de `from_unix` e em até 24 h, em que o sol cruza para baixo
/// a altitude de nascer/pôr (`SUN_RISE_ALT_DEG`). Varredura de 1 min, depois bisseção.
pub fn next_sunset(from_unix: f64) -> Option<f64> {
    let thr = SUN_RISE_ALT_DEG;
    let mut prev = from_unix;
    let mut prev_alt = sun(prev).altitude_deg;
    for i in 1..=24 * 60 {
        let t = from_unix + 60.0 * i as f64;
        let alt = sun(t).altitude_deg;
        if prev_alt >= thr && alt < thr {
            let (mut lo, mut hi) = (prev, t);
            for _ in 0..30 {
                let mid = 0.5 * (lo + hi);
                if sun(mid).altitude_deg >= thr {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            return Some(0.5 * (lo + hi));
        }
        prev = t;
        prev_alt = alt;
    }
    None
}

/// Luz global 0..1 (0 = noite escura, 1 = dia pleno). O sol domina; a lua soma pouco, em
/// proporção à fase e à altura. `weather` é o multiplicador do clima futuro (1 = céu limpo).
pub fn light_level(sun_alt_deg: f64, moon: &MoonState, weather: f64) -> f64 {
    let sun_term = ((sun_alt_deg - SUN_DARK_DEG) / (SUN_FULL_DEG - SUN_DARK_DEG)).clamp(0.0, 1.0);
    let moon_height = (moon.position.altitude_deg / MOON_FULL_ALT_DEG).clamp(0.0, 1.0);
    let moon_term = MOON_MAX_LIGHT * moon.illumination * moon_height;
    ((sun_term + moon_term) * weather).clamp(0.0, 1.0)
}

/// Interpolação linear de uma cor sobre paradas `(x, cor)` em ordem crescente de `x`.
/// Fora das paradas, devolve a cor da parada mais próxima.
pub fn gradient(stops: &[(f64, Rgb)], x: f64) -> Rgb {
    let (first, last) = (stops[0], stops[stops.len() - 1]);
    if x <= first.0 {
        return first.1;
    }
    if x >= last.0 {
        return last.1;
    }
    for pair in stops.windows(2) {
        let ((x0, c0), (x1, c1)) = (pair[0], pair[1]);
        if x <= x1 {
            let t = ((x - x0) / (x1 - x0)) as f32;
            return [0, 1, 2].map(|i| c0[i] + (c1[i] - c0[i]) * t);
        }
    }
    last.1
}

/// Cor do sol pela altura: branco-amarelado no alto, laranja perto do horizonte, azul-escuro
/// depois do ocaso.
pub fn sun_tint(sun_alt_deg: f64) -> Rgb {
    gradient(
        &[
            (-6.0, [0.12, 0.14, 0.28]),
            (0.0, [1.0, 0.48, 0.18]),
            (6.0, [1.0, 0.82, 0.60]),
            (20.0, [1.0, 0.97, 0.92]),
        ],
        sun_alt_deg,
    )
}

/// Energia da luz solar direta, 0 abaixo do crepúsculo civil e `max` ao meio-dia.
pub fn sun_energy(sun_alt_deg: f64, max: f64) -> f64 {
    max * ((sun_alt_deg - SUN_DARK_DEG) / (SUN_FULL_DEG - SUN_DARK_DEG)).clamp(0.0, 1.0)
}

/// Cor do horizonte do céu (também a cor da neblina), pela altura do sol.
pub fn horizon_tint(sun_alt_deg: f64) -> Rgb {
    gradient(
        &[
            (-12.0, [0.05, 0.07, 0.14]),
            (-4.0, [0.36, 0.24, 0.40]),
            (1.0, [1.0, 0.50, 0.20]),
            (12.0, [0.90, 0.70, 0.60]),
            (25.0, [0.62, 0.82, 0.98]),
        ],
        sun_alt_deg,
    )
}

/// Cor do topo do céu, pela altura do sol.
pub fn zenith_tint(sun_alt_deg: f64) -> Rgb {
    gradient(
        &[
            (-12.0, [0.01, 0.02, 0.06]),
            (-4.0, [0.08, 0.10, 0.22]),
            (1.0, [0.30, 0.34, 0.60]),
            (12.0, [0.18, 0.36, 0.78]),
            (25.0, [0.22, 0.46, 0.92]),
        ],
        sun_alt_deg,
    )
}

/// Limiar de luz com histerese: o sinal "descendo" dispara ao cair abaixo de `on_below`;
/// o sinal "subindo" dispara ao passar de `off_above`. Entre os dois, nada acontece.
#[derive(Clone, Debug, PartialEq)]
pub struct LightThreshold {
    pub name: String,
    pub on_below: f64,
    pub off_above: f64,
}

/// Cruzamento de um limiar. `descending` = a luz desceu até `on_below`; `false` = subiu
/// acima de `off_above`.
#[derive(Clone, Debug, PartialEq)]
pub struct Crossing {
    pub name: String,
    pub descending: bool,
}

/// Registro de limiares e o estado de histerese de cada um.
#[derive(Default, Debug)]
pub struct ThresholdBank {
    thresholds: Vec<LightThreshold>,
    /// `None` até a primeira amostra; depois, `true` se o último sinal foi "descendo".
    below: Vec<Option<bool>>,
}

impl ThresholdBank {
    /// Registra um limiar. Panics se `on_below >= off_above` (a histerese precisa de faixa).
    pub fn add(&mut self, threshold: LightThreshold) {
        assert!(
            threshold.on_below < threshold.off_above,
            "threshold '{}': on_below must be below off_above",
            threshold.name
        );
        self.thresholds.push(threshold);
        self.below.push(None);
    }

    #[allow(dead_code)] // API for consumers in later slices
    pub fn thresholds(&self) -> &[LightThreshold] {
        &self.thresholds
    }

    /// Avança com uma nova amostra de luz e devolve só os cruzamentos desta amostra.
    /// A primeira amostra fixa o estado inicial sem emitir sinal.
    pub fn update(&mut self, light: f64) -> Vec<Crossing> {
        let mut out = Vec::new();
        for (t, state) in self.thresholds.iter().zip(self.below.iter_mut()) {
            match *state {
                None => *state = Some(light < t.on_below),
                Some(false) if light < t.on_below => {
                    *state = Some(true);
                    out.push(Crossing {
                        name: t.name.clone(),
                        descending: true,
                    });
                }
                Some(true) if light > t.off_above => {
                    *state = Some(false);
                    out.push(Crossing {
                        name: t.name.clone(),
                        descending: false,
                    });
                }
                _ => {}
            }
        }
        out
    }
}

/// Atraso fixo de um consumidor, em minutos de jogo de 0 a `max_min` (inclusive), pelo hash
/// FNV-1a do seu id. Somado ao instante do sinal, espalha os acendimentos no tempo.
#[allow(dead_code)] // API for consumers (lamps, windows) in later slices
pub fn stagger_delay_min(id: &str, max_min: u32) -> f64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in id.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (h % (max_min as u64 + 1)) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sunset_local_hour_2026_11_15() -> f64 {
        let from = local_to_unix(2026, 11, 15, 0.0);
        local_hour(next_sunset(from).expect("sunset found"))
    }

    #[test]
    fn unix_epoch_and_local_time() {
        assert_eq!(local_to_unix(1970, 1, 1, 0.0), LOCAL_UTC_OFFSET_S * -1.0);
        let t = local_to_unix(2026, 11, 15, 17.5);
        assert!((local_hour(t) - 17.5).abs() < 1e-9);
    }

    #[test]
    fn sunset_bh_15_nov_2026_is_around_1805_to_1820() {
        // Conta à mão (Meeus, dec -18,9, lat -19,9, equação do tempo ~+14 min, refração):
        // ocaso ~18:14 BRT. A nota de backlog diz 17:50-18:00: divergência em aberto.
        let h = sunset_local_hour_2026_11_15();
        assert!(
            (18.0 + 5.0 / 60.0..18.0 + 20.0 / 60.0).contains(&h),
            "sunset at local hour {h}"
        );
    }

    #[test]
    fn sunset_azimuth_is_west_southwest() {
        // cos(A) = sin(dec) / cos(lat) no horizonte: com dec ~ -18,9 grau em 15/11, A ~ 249,8.
        // A nota de backlog diz ~260 +-5: divergência em aberto (ver relatório).
        let s = next_sunset(local_to_unix(2026, 11, 15, 0.0)).unwrap();
        let az = sun(s).azimuth_deg;
        assert!((248.0..252.0).contains(&az), "sunset azimuth {az}");
    }

    #[test]
    fn noon_sun_is_high_and_north() {
        let base = local_to_unix(2026, 11, 15, 0.0);
        let (mut best_alt, mut best_az, mut best_h) = (-90.0_f64, 0.0, 0.0);
        for m in 0..(24 * 60) {
            let s = sun(base + 60.0 * m as f64);
            if s.altitude_deg > best_alt {
                best_alt = s.altitude_deg;
                best_az = s.azimuth_deg;
                best_h = local_hour(base + 60.0 * m as f64);
            }
        }
        assert!((11.5..12.5).contains(&best_h), "solar noon at {best_h} h");
        assert!(best_alt > 45.0, "noon altitude {best_alt}");
        // Sol ao meio-dia no hemisfério sul de novembro fica ao norte.
        assert!(!(15.0..345.0).contains(&best_az), "noon azimuth {best_az}");
    }

    #[test]
    fn full_moon_and_new_moon_phase() {
        // Lua cheia 2024-01-25 17:54 UTC; lua nova 2024-01-11 11:57 UTC.
        let full = 1706205240.0_f64; // 2024-01-25 17:54 UTC
        let new = 1704974220.0_f64; // 2024-01-11 11:57 UTC
        assert!(
            moon(full).illumination > 0.98,
            "full {}",
            moon(full).illumination
        );
        assert!(
            moon(new).illumination < 0.02,
            "new {}",
            moon(new).illumination
        );
    }

    #[test]
    fn light_is_dominated_by_sun_and_moon_adds_little() {
        let night_moon = MoonState {
            position: Horizontal {
                azimuth_deg: 0.0,
                altitude_deg: 60.0,
            },
            illumination: 1.0,
            waxing: true,
        };
        let noon = light_level(60.0, &night_moon, 1.0);
        assert_eq!(noon, 1.0);
        let dark_full = light_level(-30.0, &night_moon, 1.0);
        assert!(
            (dark_full - MOON_MAX_LIGHT).abs() < 1e-9,
            "moonlight {dark_full}"
        );
        let no_moon = MoonState {
            illumination: 0.0,
            ..night_moon
        };
        assert_eq!(light_level(-30.0, &no_moon, 1.0), 0.0);
        assert_eq!(light_level(60.0, &no_moon, 0.5), 0.5);
    }

    #[test]
    fn hysteresis_emits_only_on_crossings() {
        let mut bank = ThresholdBank::default();
        bank.add(LightThreshold {
            name: "lamp".into(),
            on_below: 0.4,
            off_above: 0.5,
        });
        assert!(bank.update(0.9).is_empty()); // primeira amostra: estado inicial, sem sinal
        assert!(bank.update(0.6).is_empty());
        let down = bank.update(0.39);
        assert_eq!(
            down,
            vec![Crossing {
                name: "lamp".into(),
                descending: true
            }]
        );
        // Oscilação dentro da faixa de histerese não dispara nada.
        assert!(bank.update(0.45).is_empty());
        assert!(bank.update(0.41).is_empty());
        assert!(bank.update(0.3).is_empty());
        let up = bank.update(0.51);
        assert_eq!(
            up,
            vec![Crossing {
                name: "lamp".into(),
                descending: false
            }]
        );
        assert!(bank.update(0.55).is_empty());
    }

    #[test]
    fn initial_dark_state_does_not_fire_then_rises() {
        let mut bank = ThresholdBank::default();
        bank.add(LightThreshold {
            name: "x".into(),
            on_below: 0.4,
            off_above: 0.5,
        });
        assert!(bank.update(0.1).is_empty());
        assert_eq!(bank.update(0.6).len(), 1);
    }

    #[test]
    #[should_panic]
    fn threshold_without_band_is_rejected() {
        let mut bank = ThresholdBank::default();
        bank.add(LightThreshold {
            name: "bad".into(),
            on_below: 0.5,
            off_above: 0.5,
        });
    }

    #[test]
    fn stagger_is_stable_and_bounded() {
        let a = stagger_delay_min("poste-12", 5);
        assert_eq!(a, stagger_delay_min("poste-12", 5));
        let mut seen = std::collections::HashSet::new();
        for i in 0..500 {
            let d = stagger_delay_min(&format!("poste-{i}"), 5);
            assert!((0.0..=5.0).contains(&d));
            seen.insert(d as u32);
        }
        assert_eq!(seen.len(), 6, "all 0..=5 minute delays appear");
    }
}
