//! Astronomia e luz do ambiente, pura (sem Godot): posição do sol e da lua, fase, luz global,
//! limiares com histerese e atraso por id. Ângulos em graus, tempo em segundos Unix (UTC),
//! distâncias em metros. Fórmulas de baixa precisão (Meeus, cap. 25 e 47), suficientes para
//! uma cena: erro do sol de ~0,01 grau e da lua de ~0,3 grau.

use std::f64::consts::{PI, TAU};
use std::sync::Mutex;

use crate::style::{self, LightLook};

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

/// Home of the sky until the player saves one: Praça Sete, Belo Horizonte (public, generic).
const DEFAULT_HOME_DEG: (f64, f64) = (-19.9191, -43.9386);
/// Home (lat, lon) in degrees that the sun and moon are computed for. Set from the saved home.
static SKY_HOME: Mutex<(f64, f64)> = Mutex::new(DEFAULT_HOME_DEG);

/// Sets the home of the sky (sun and moon), in degrees.
pub fn set_home(lat_deg: f64, lon_deg: f64) {
    if let Ok(mut home) = SKY_HOME.lock() {
        *home = (lat_deg, lon_deg);
    }
}

const SECONDS_PER_DAY: f64 = 86400.0;
/// Dia Juliano do instante Unix 0.
const JD_UNIX_EPOCH: f64 = 2440587.5;
/// Dia Juliano de J2000.0.
const JD_J2000: f64 = 2451545.0;
/// Obliquidade da eclíptica em J2000, graus.
const OBLIQUITY_J2000_DEG: f64 = 23.439291;
/// Fração do laranja do ocaso que aparece fora do lado do sol (espelha `WARM_HAZE` do shader).
const WARM_HAZE: f32 = 0.35;

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

/// Data civil local (ano, mês, dia) de um instante Unix em Belo Horizonte.
pub fn local_date(unix_s: f64) -> (i64, u32, u32) {
    let days = ((unix_s + LOCAL_UTC_OFFSET_S) / SECONDS_PER_DAY).floor() as i64;
    civil_from_days(days)
}

/// Data civil (ano, mês, dia) de `days` desde 1970-01-01 (inverso de `days_from_civil`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
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

/// Converte ascensão reta e declinação (radianos) em azimute/altura para a posição de casa.
fn horizontal(jd: f64, ra_rad: f64, dec_rad: f64) -> Horizontal {
    let (home_lat, home_lon) = SKY_HOME.lock().map(|h| *h).unwrap_or(DEFAULT_HOME_DEG);
    let lat = home_lat.to_radians();
    let gmst_deg = (280.46061837 + 360.98564736629 * (jd - JD_J2000)).rem_euclid(360.0);
    let ha = (gmst_deg + home_lon - ra_rad.to_degrees()).to_radians();
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
    next_horizon_crossing(from_unix, false)
}

/// Primeiro instante, a partir de `from_unix` e em até 24 h, em que o sol cruza para cima
/// a altitude de nascer/pôr (`SUN_RISE_ALT_DEG`).
pub fn next_sunrise(from_unix: f64) -> Option<f64> {
    next_horizon_crossing(from_unix, true)
}

/// Primeiro cruzamento da altitude de nascer/pôr após `from_unix`: para cima se `rising`,
/// senão para baixo. Varredura de 1 min, depois bisseção.
fn next_horizon_crossing(from_unix: f64, rising: bool) -> Option<f64> {
    let thr = SUN_RISE_ALT_DEG;
    // A crossing ends on the side the search looks for: above the threshold if rising, below if not.
    let crosses = |alt: f64, prev_alt: f64| {
        if rising {
            prev_alt < thr && alt >= thr
        } else {
            prev_alt >= thr && alt < thr
        }
    };
    let mut prev = from_unix;
    let mut prev_alt = sun(prev).altitude_deg;
    for i in 1..=24 * 60 {
        let t = from_unix + 60.0 * i as f64;
        let alt = sun(t).altitude_deg;
        if crosses(alt, prev_alt) {
            let (mut lo, mut hi) = (prev, t);
            for _ in 0..30 {
                let mid = 0.5 * (lo + hi);
                if (sun(mid).altitude_deg >= thr) == rising {
                    hi = mid;
                } else {
                    lo = mid;
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

/// Share of the sun and sky light lost under full cloud cover (0 clear, 1 overcast).
pub const CLOUD_LIGHT_LOSS: f64 = 0.75;

/// Light multiplier of the cloud cover: 1 with a clear sky, `1 - CLOUD_LIGHT_LOSS` under full cover.
/// `cover` is clamped to 0..1.
pub fn cloud_light_factor(cover: f64) -> f64 {
    1.0 - CLOUD_LIGHT_LOSS * cover.clamp(0.0, 1.0)
}

/// Passo suave de Hermite (como o `smoothstep` do GLSL); aceita `e0 > e1` (passo invertido).
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp3(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

fn clamp01(c: Rgb) -> Rgb {
    c.map(|v| v.clamp(0.0, 1.0))
}

/// sRGB channels (0..1) to linear RGB, the space of the sky shader colors.
pub fn srgb_to_linear(c: Rgb) -> Rgb {
    c.map(|v| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    })
}

/// Linear RGB (0..1) to sRGB, for the colors Godot receives as `Color` (ambient light).
pub fn linear_to_srgb(c: Rgb) -> Rgb {
    c.map(|v| {
        if v <= 0.003_130_8 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        }
    })
}

/// One row of the time-of-day light table: a human moment, the sun altitude where it applies and
/// its colors. Sky colors are linear RGB (the shader space); `sun` is sRGB (a Godot light color).
///
/// The table is keyed by sun altitude because that is what the sky responds to. The moments and
/// what each one must look like (item 121):
///
/// | Moment           | Sun altitude | Look |
/// |------------------|--------------|------|
/// | night            | below -12    | deep navy zenith, warm city glow, moon and lamps as before |
/// | blue hour        | -12 .. -6    | deep blues with a coral afterglow on the sunset side |
/// | sunrise / sunset | -6 .. 0      | peach-orange sun (`sun_low`), soft contrast, long shadows (cool at dawn, warm at dusk) |
/// | golden hour      | 0 .. 8       | orange/coral glow, long warm shadows, pale blue zenith |
/// | morning          | 8 .. 30      | warm white sun (`sun_high`), clear saturated blue sky, crisp shadows |
/// | noon             | 30 .. 70     | neutral-warm white sun, deep blue zenith, pale hazy horizon, high but unclipped contrast, short blue-violet shadows |
/// | afternoon        | any, west    | the same altitude sampled ~4 deg lower plus a warm sun bias: golden |
/// | apex             | 70 .. 90     | the sun overhead; energy and sky keep rising instead of clamping |
///
/// The afternoon is a direction, not an altitude: `afternoon_warmth` turns the sun's hour angle
/// into a 0..1 weight and the table is sampled that much lower, so 15:30 is warmer than 09:00 at
/// the same sun height. Theme anchors: `sun_high` is the sun at noon, `sun_low` the sun at the
/// horizon, `sky_day` the deep blue of the zenith, `sky_horizon` the pale horizon and
/// `shadow_tint` feeds the ambient fill (see `ambient_tint`).
// `name` and `description` document the table for a human reader and are asserted by the tests;
// the frame path only reads the colors and the altitude.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub struct Moment {
    pub name: &'static str,
    pub description: &'static str,
    /// Sun altitude of the row, in degrees.
    pub key_alt_deg: f64,
    pub zenith: Rgb,
    pub mid: Rgb,
    pub horizon: Rgb,
    pub sun: Rgb,
    pub glow: Rgb,
    pub glow_hi: Rgb,
    /// How warm the ambient fill is on this row: 0 blue-violet (cool shadows), 1 warm gray
    /// (golden-hour shadows). See `ambient_tint`.
    pub ambient_warmth: f32,
}

/// Builds the seven rows of the table from the theme anchors. Called per frame; it is only a
/// handful of color mixes, no allocation.
pub fn moment_table(anchors: &LightLook) -> [Moment; 7] {
    let day = srgb_to_linear(anchors.sky_day);
    let pale = srgb_to_linear(anchors.sky_horizon);
    // The item 124 fixes target the saturated warm horizon of the lowpoly family. The older
    // themes (the realistic preset) have a pale, almost neutral horizon and keep the previous
    // table, so their appearance does not change.
    let warm_horizon = warm_horizon(anchors);
    // Deep zenith: the theme's day blue with the low channels pulled down. The top of the sky
    // stays saturated instead of fading into the horizon's pale gray.
    let deep = clamp01([0.58 * day[0], 0.70 * day[1], 1.12 * day[2]]);
    let apex = clamp01([deep[0] * 1.06, deep[1] * 1.05, deep[2] * 1.06]);
    let warm_white: Rgb = [1.0, 0.96, 0.90];
    [
        Moment {
            name: "night",
            description: "Deep navy sky with a warm city glow at the horizon; the moon and the street lamps light the scene.",
            key_alt_deg: -18.0,
            zenith: [0.006, 0.012, 0.036],
            mid: [0.014, 0.024, 0.060],
            horizon: [0.050, 0.055, 0.090],
            sun: [0.10, 0.12, 0.24],
            glow: [0.30, 0.16, 0.24],
            glow_hi: [0.40, 0.26, 0.38],
            ambient_warmth: 0.0,
        },
        Moment {
            name: "blue hour",
            description: "Deep blues with a coral afterglow on the sunset side; the first lamps and windows come on.",
            key_alt_deg: -6.0,
            zenith: [0.045, 0.080, 0.190],
            mid: [0.180, 0.200, 0.360],
            horizon: [0.240, 0.250, 0.380],
            sun: [0.35, 0.20, 0.28],
            glow: [0.95, 0.38, 0.30],
            glow_hi: [0.95, 0.70, 0.45],
            ambient_warmth: 0.15,
        },
        Moment {
            name: "sunrise / sunset",
            description: "Peach-orange sun, soft contrast and long shadows (cool at dawn, warm at dusk); the zenith is still petrol blue.",
            key_alt_deg: -0.5,
            zenith: [0.080, 0.210, 0.360],
            mid: [0.490, 0.445, 0.580],
            horizon: [0.480, 0.435, 0.570],
            sun: lerp3(anchors.sun_low, [1.0, 0.42, 0.24], 0.35),
            glow: [1.0, 0.42, 0.15],
            glow_hi: [1.0, 0.70, 0.35],
            ambient_warmth: 0.85,
        },
        Moment {
            name: "golden hour",
            description: "Orange/coral glow, long warm shadows and a pale blue zenith; the sun is warm white with a golden edge.",
            key_alt_deg: 6.0,
            zenith: [0.090, 0.230, 0.540],
            mid: [0.500, 0.480, 0.640],
            horizon: [0.620, 0.580, 0.680],
            sun: lerp3(anchors.sun_low, anchors.sun_high, 0.45),
            glow: [1.0, 0.52, 0.20],
            glow_hi: [1.0, 0.82, 0.45],
            ambient_warmth: 0.85,
        },
        Moment {
            name: "morning",
            description: "Warm white sun, a clear saturated blue sky and crisp shadows; contrast is already high.",
            key_alt_deg: 14.0,
            zenith: clamp01([0.52 * day[0], 0.63 * day[1], 0.91 * day[2]]),
            // The mid band is the day blue lightened toward warm white, not the peach of the
            // horizon: the morning is bright without tinting the shaded faces.
            mid: if warm_horizon {
                lerp3(day, warm_white, 0.26)
            } else {
                lerp3(day, pale, 0.24)
            },
            // The horizon stays the pale peach of the theme instead of mixing back toward the
            // darker day blue: the morning is bright, not a second golden hour.
            horizon: if warm_horizon {
                lerp3(pale, warm_white, 0.05)
            } else {
                lerp3(pale, day, 0.08)
            },
            sun: lerp3(anchors.sun_low, anchors.sun_high, 0.80),
            glow: [1.0, 0.72, 0.50],
            glow_hi: [1.0, 0.92, 0.75],
            ambient_warmth: 0.05,
        },
        Moment {
            name: "noon",
            description: "Neutral-warm white sun, deep blue zenith and a pale hazy horizon; high contrast without clipping, short blue-violet shadows.",
            key_alt_deg: 45.0,
            zenith: deep,
            mid: if warm_horizon {
                lerp3(day, warm_white, 0.34)
            } else {
                lerp3(day, pale, 0.32)
            },
            // A pale peach horizon (60% of the theme horizon toward warm white), not a saturated
            // blue day: the noon is bright without turning the sky into a postcard blue.
            horizon: if warm_horizon {
                lerp3(pale, warm_white, 0.60)
            } else {
                lerp3(pale, warm_white, 0.17)
            },
            sun: anchors.sun_high,
            glow: [1.0, 0.80, 0.62],
            glow_hi: [1.0, 0.95, 0.85],
            ambient_warmth: 0.0,
        },
        Moment {
            name: "apex",
            description: "The sun overhead: the table keeps brightening instead of clamping at 6 degrees; the zenith is at its deepest.",
            key_alt_deg: 90.0,
            zenith: apex,
            mid: if warm_horizon {
                lerp3(day, warm_white, 0.40)
            } else {
                lerp3(day, pale, 0.35)
            },
            horizon: if warm_horizon {
                lerp3(pale, warm_white, 0.62)
            } else {
                lerp3(pale, warm_white, 0.20)
            },
            sun: anchors.sun_high,
            glow: [1.0, 0.82, 0.66],
            glow_hi: [1.0, 0.96, 0.88],
            ambient_warmth: 0.0,
        },
    ]
}

/// Interpolated colors at one sun altitude.
#[derive(Clone, Copy, Debug)]
struct Sample {
    zenith: Rgb,
    mid: Rgb,
    horizon: Rgb,
    sun: Rgb,
    glow: Rgb,
    glow_hi: Rgb,
    ambient_warmth: f32,
}

impl Sample {
    fn of(m: &Moment) -> Self {
        Self {
            zenith: m.zenith,
            mid: m.mid,
            horizon: m.horizon,
            sun: m.sun,
            glow: m.glow,
            glow_hi: m.glow_hi,
            ambient_warmth: m.ambient_warmth,
        }
    }
}

fn sample_table(table: &[Moment; 7], alt_deg: f64) -> Sample {
    let clamped = alt_deg.clamp(table[0].key_alt_deg, table[6].key_alt_deg);
    for pair in table.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if clamped <= b.key_alt_deg {
            let t = ((clamped - a.key_alt_deg) / (b.key_alt_deg - a.key_alt_deg)) as f32;
            return Sample {
                zenith: lerp3(a.zenith, b.zenith, t),
                mid: lerp3(a.mid, b.mid, t),
                horizon: lerp3(a.horizon, b.horizon, t),
                sun: lerp3(a.sun, b.sun, t),
                glow: lerp3(a.glow, b.glow, t),
                glow_hi: lerp3(a.glow_hi, b.glow_hi, t),
                ambient_warmth: a.ambient_warmth + (b.ambient_warmth - a.ambient_warmth) * t,
            };
        }
    }
    Sample::of(&table[6])
}

/// Effective altitude lost by the afternoon sun in the table, in degrees.
const AFTERNOON_SHIFT_DEG: f64 = 4.0;
/// How far the afternoon sun color is pulled toward `sun_low`, 0..1 of the gap.
const AFTERNOON_SUN_WARMTH: f32 = 0.10;
/// Hour angle where the afternoon warm bias starts and where it is full, in degrees.
const AFTERNOON_HA_START_DEG: f32 = 5.0;
const AFTERNOON_HA_FULL_DEG: f32 = 45.0;

/// Hour angle of the sun, in degrees (-180..180): negative before solar noon, positive after it.
/// Uses the same low-precision solar position as `sun`, so it works in both hemispheres.
pub fn sun_hour_angle_deg(unix_s: f64) -> f64 {
    let jd = julian_day(unix_s);
    let lon = sun_ecliptic_lon_rad(jd);
    let eps = obliquity_rad(jd);
    let ra_deg = (eps.cos() * lon.sin()).atan2(lon.cos()).to_degrees();
    let (_, home_lon) = SKY_HOME.lock().map(|h| *h).unwrap_or(DEFAULT_HOME_DEG);
    let gmst_deg = (280.46061837 + 360.98564736629 * (jd - JD_J2000)).rem_euclid(360.0);
    (gmst_deg + home_lon - ra_deg + 180.0).rem_euclid(360.0) - 180.0
}

/// Afternoon weight 0..1 from the hour angle: 0 until 5 degrees past solar noon, 1 by 45 degrees.
pub fn afternoon_warmth(hour_angle_deg: f64) -> f32 {
    smooth(
        AFTERNOON_HA_START_DEG,
        AFTERNOON_HA_FULL_DEG,
        hour_angle_deg as f32,
    )
}

fn look_colors(anchors: &LightLook, alt_deg: f64, afternoon: f32) -> Sample {
    let afternoon = afternoon.clamp(0.0, 1.0);
    let table = moment_table(anchors);
    let mut sample = sample_table(&table, alt_deg - AFTERNOON_SHIFT_DEG * f64::from(afternoon));
    if afternoon > 0.0 {
        sample.sun = lerp3(
            sample.sun,
            anchors.sun_low,
            AFTERNOON_SUN_WARMTH * afternoon,
        );
    }
    // The same low sun has cool shadows at dawn and warm ones at dusk. Use the unshifted row for
    // the warmth (the afternoon shift would push the sunset row down into the blue hour) and let
    // the warm fill of the golden rows apply only on the afternoon side.
    let warmth = sample_table(&table, alt_deg).ambient_warmth;
    sample.ambient_warmth = warmth * (0.25 + 0.75 * afternoon);
    sample
}

/// Sun color by altitude, morning side, sRGB.
// Kept as a table accessor for tests and later lighting slices; the frame path reads `SkyLook`.
#[allow(dead_code)]
pub fn sun_tint(sun_alt_deg: f64) -> Rgb {
    sun_tint_at(sun_alt_deg, 0.0)
}

/// Sun color by altitude with the afternoon warm bias 0..1, sRGB.
#[allow(dead_code)]
pub fn sun_tint_at(sun_alt_deg: f64, afternoon: f32) -> Rgb {
    look_colors(&style::light_look(), sun_alt_deg, afternoon).sun
}

/// Share of the sun energy left at the horizon; the air-mass term supplies the rest.
const SUN_HORIZON_ENERGY: f64 = 0.45;

/// Direct sun energy at the zenith (Godot's default is ~1). Below the old 1.6 so the pale
/// concrete fits the ACES shoulder at the higher exposure of the low-sun fill.
pub const SUN_MAX_ENERGY: f64 = 1.20;

/// Direct sun energy: zero at civil twilight, a steep sunrise ramp, then the air-mass term keeps
/// rising all the way to the zenith (it is not clamped at `SUN_FULL_DEG`).
pub fn sun_energy(sun_alt_deg: f64, max: f64) -> f64 {
    let twilight = ((sun_alt_deg - SUN_DARK_DEG) / (0.0 - SUN_DARK_DEG)).clamp(0.0, 1.0);
    let air_mass = if sun_alt_deg > 0.0 {
        sun_alt_deg.to_radians().sin().max(0.0).sqrt()
    } else {
        0.0
    };
    max * twilight * (SUN_HORIZON_ENERGY + (1.0 - SUN_HORIZON_ENERGY) * air_mass)
}

/// Ambient fill energy at night, when the sun is below civil twilight.
pub const AMBIENT_NIGHT: f64 = 0.58;
/// Ambient fill energy at low sun (below ~10 deg): sunrise/sunset must stay readable.
pub const AMBIENT_LOW: f64 = 1.20;
/// Ambient fill energy with the sun high (above ~45 deg): the direct sun keeps the contrast and
/// the blue-violet shadows, so the fill stays low on purpose.
pub const AMBIENT_DAY: f64 = 0.62;

/// Ambient light energy by sun altitude. It is not one `sqrt(light)` ramp: the low sun needs a lot
/// of fill (the direct light is weak and warm), the high sun very little (that is what keeps the
/// noon from flattening into the hospital gray), and the night a floor that keeps the lamps and
/// the city readable.
pub fn ambient_energy(sun_alt_deg: f64) -> f64 {
    let alt = sun_alt_deg as f32;
    let day = smooth(-6.0, 2.0, alt) as f64;
    let low = 1.0 - smooth(10.0, 45.0, alt) as f64;
    let fill = AMBIENT_DAY + (AMBIENT_LOW - AMBIENT_DAY) * low;
    AMBIENT_NIGHT + (fill - AMBIENT_NIGHT) * day
}

/// Tonemapper exposure with a full day sky (the night stays at 1 in `clock.rs`).
pub const DAY_TONEMAP_EXPOSURE: f32 = 0.62;
/// Extra exposure dip as the sun climbs to the zenith, where the pale concrete has no highlight
/// headroom left.
pub const HIGH_SUN_EXPOSURE_DIP: f32 = 0.17;
/// Extra exposure loss at a clear high sun, where the direct sun on the pale concrete is
/// strongest. The loss fades out below 45 deg, where there is no clipping and the sunset and the
/// morning must stay bright.
pub const CLEAR_SKY_EXPOSURE_LOSS: f32 = 0.55;

/// Day tonemapper exposure for a sun altitude and cloud cover, 0..1. A clear sky at high sun gets
/// less exposure than an overcast one because its sun is much stronger; `scene_key` checks that
/// the resulting image is still brighter, which is the target (a clear noon must not read darker).
pub fn day_exposure(sun_alt_deg: f64, cloud_cover: f64) -> f32 {
    let high_sun = ((sun_alt_deg as f32 - 45.0) / 40.0).clamp(0.0, 1.0);
    let clear = (1.0 - cloud_cover.clamp(0.0, 1.0) as f32).powi(2);
    let clear_loss = CLEAR_SKY_EXPOSURE_LOSS * high_sun * clear;
    (DAY_TONEMAP_EXPOSURE - HIGH_SUN_EXPOSURE_DIP * high_sun) * (1.0 - clear_loss)
}

/// Relative brightness of the lit scene (sun + ambient fill) after the exposure: the sun term is
/// the direct light on a horizontal surface, the ambient term a share of the sky fill. Used by the
/// tests to keep a clear noon brighter than an overcast one.
// Used by the lightness regression tests; the frame path sets the exposure directly.
#[allow(dead_code)]
pub fn scene_key(sun_alt_deg: f64, cloud_cover: f64) -> f64 {
    let sun = sun_energy(sun_alt_deg, SUN_MAX_ENERGY) * cloud_light_factor(cloud_cover);
    let ambient = ambient_energy(sun_alt_deg) * 0.65;
    day_exposure(sun_alt_deg, cloud_cover) as f64 * (sun + ambient)
}

/// Extra sky brightness with a clear sky at high sun, 0..1 of the gap: the cloud veil already
/// grays an overcast sky, and this keeps a clear noon from reading darker than the overcast one
/// after the clear-sky exposure loss.
pub const SKY_CLEAR_GAIN: f32 = 0.75;

/// Sky brightness gain by sun altitude and cloud cover. It is 1 at night, below 45 deg and under
/// full overcast, so only the clear high-sun sky is lifted.
pub fn sky_gain(sun_alt_deg: f64, cloud_cover: f64) -> f32 {
    let high_sun = ((sun_alt_deg as f32 - 45.0) / 40.0).clamp(0.0, 1.0);
    let clear = (1.0 - cloud_cover.clamp(0.0, 1.0) as f32).powi(2);
    1.0 + SKY_CLEAR_GAIN * high_sun * clear
}

/// Horizon color on the side opposite the sun, linear RGB (also the base of the depth fog).
#[allow(dead_code)]
pub fn horizon_tint(sun_alt_deg: f64) -> Rgb {
    look_colors(&style::light_look(), sun_alt_deg, 0.0).horizon
}

/// Intermediate sky band (between horizon and zenith) on the side opposite the sun, linear RGB.
#[allow(dead_code)]
pub fn mid_tint(sun_alt_deg: f64) -> Rgb {
    look_colors(&style::light_look(), sun_alt_deg, 0.0).mid
}

/// Zenith color, linear RGB: deep and saturated at noon, petrol blue at dusk, navy at night.
#[allow(dead_code)]
pub fn zenith_tint(sun_alt_deg: f64) -> Rgb {
    look_colors(&style::light_look(), sun_alt_deg, 0.0).zenith
}

/// Low glow band on the sunset side, linear RGB.
#[allow(dead_code)]
pub fn glow_tint(sun_alt_deg: f64) -> Rgb {
    look_colors(&style::light_look(), sun_alt_deg, 0.0).glow
}

/// Upper glow band on the sunset side, linear RGB.
#[allow(dead_code)]
pub fn glow_hi_tint(sun_alt_deg: f64) -> Rgb {
    look_colors(&style::light_look(), sun_alt_deg, 0.0).glow_hi
}

/// Gain that turns the sky luminance into the brightness of the cloud veil (same in the shader):
/// clouds are lit by the sky, so a cloudy night stays dark and a cloudy noon stays light gray.
pub const CLOUD_LIFT_GAIN: f32 = 2.5;

/// Warm gray of a fully overcast sky, linear RGB: a near-neutral gray at the luminance of the
/// pale horizon anchor, with only a hint of red, so an overcast noon reads as natural warm gray
/// instead of flat white, blue or the saturated peach of the clear horizon. The old formula mixed
/// 30% of the horizon into a gray; with the lowpoly peach that stayed as saturated as the clear
/// sky and defeated the veil.
pub fn cloud_tint(anchors: &LightLook) -> Rgb {
    let pale = srgb_to_linear(anchors.sky_horizon);
    if !warm_horizon(anchors) {
        // Older themes keep the previous gray mix, so the realistic preset is unchanged.
        return lerp3(pale, [0.435, 0.404, 0.352], 0.70);
    }
    let gray = 0.2126 * pale[0] + 0.7152 * pale[1] + 0.0722 * pale[2];
    [(gray * 1.005).min(1.0), gray, (gray * 0.992).min(1.0)]
}

/// Minimum saturation of the horizon anchor that selects the warm lowpoly family: the lowpoly
/// peach is ~0.43, the realistic near-white ~0.12.
const WARM_HORIZON_MIN_SAT: f32 = 0.2;

/// True when the horizon anchor is a saturated warm peach (the lowpoly family) instead of the
/// pale, almost neutral horizon of the older themes. The item 124 table and overcast fixes only
/// apply to that family; `realistic.json` keeps the previous look.
fn warm_horizon(anchors: &LightLook) -> bool {
    let pale = srgb_to_linear(anchors.sky_horizon);
    let max = pale[0].max(pale[1]).max(pale[2]);
    let min = pale[0].min(pale[1]).min(pale[2]);
    max > 0.0 && (max - min) / max > WARM_HORIZON_MIN_SAT
}

/// Warm haze of the item 124 lowpoly atmosphere (#E8B49C: its sRGB channels are used directly as
/// the shader color, as item 124 set them) and its share of the depth fog color.
pub const ATMOSPHERE_HAZE_RGB: Rgb = [0.910, 0.706, 0.612];
pub const ATMOSPHERE_HAZE_MIX: f32 = 0.45;

/// Luminance of a linear RGB color (Rec. 709 weights), the same the shader uses.
fn luma(c: Rgb) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Color of the weather fog at full density — and of the sky it washes — linear RGB: the overcast
/// gray of the cloud veil (`cloud_tint`) at the horizon's brightness, warmed toward the atmosphere
/// haze when the theme has it. Scaling by the horizon's own brightness keeps a foggy night as dark
/// as the overcast sky it hides behind instead of letting the warm haze glow.
pub fn haze_color(look: &SkyLook, atmosphere: bool) -> Rgb {
    let lift = (luma(look.horizon) * CLOUD_LIFT_GAIN).clamp(0.0, 1.0);
    let gray = look.cloud_tint.map(|v| (v * lift).min(1.0));
    if atmosphere {
        let warm = ATMOSPHERE_HAZE_RGB.map(|v| v * lift);
        lerp3(gray, warm, ATMOSPHERE_HAZE_MIX)
    } else {
        gray
    }
}

/// Fraction of the theme's shadow tint mixed into the ambient fill.
const AMBIENT_SHADOW_MIX: f32 = 0.40;
/// Warm gray target of the golden-hour ambient fill, sRGB.
const AMBIENT_WARM: Rgb = [0.60, 0.50, 0.45];

/// Ambient fill color for the faces the sun does not reach, sRGB (a Godot light color): the sky's
/// mid color, kept saturated, blended toward the theme's `shadow_tint` and pushed toward blue and
/// a touch of red, so shadows read blue-violet instead of neutral gray. `warm` 0..1 moves the fill
/// toward a warm gray for the golden hour, whose shadows are long and warm.
pub fn ambient_tint(mid: Rgb, shadow_tint: Rgb, warm: f32, fill: Option<Rgb>) -> Rgb {
    let sky = linear_to_srgb(mid);
    let violet = [
        (shadow_tint[0] * 1.06).clamp(0.0, 1.0),
        (shadow_tint[1] * 0.75).clamp(0.0, 1.0),
        (shadow_tint[2] * 1.12).clamp(0.0, 1.0),
    ];
    let mut tint = lerp3(sky, violet, AMBIENT_SHADOW_MIX);
    // The cold fill of the theme (`light.fill`) pulls the shaded faces toward its blue, so the
    // shadows of the lowpoly theme read cool and lilac instead of a dark gray-brown.
    if let Some(cold) = fill {
        tint = lerp3(tint, cold, AMBIENT_FILL_MIX);
    }
    lerp3(tint, AMBIENT_WARM, warm.clamp(0.0, 1.0))
}

/// Share of the theme's cold fill mixed into the ambient fill, 0..1.
const AMBIENT_FILL_MIX: f32 = 0.30;

/// Parâmetros do céu para o shader (`godot/shaders/sky.gdshader`) e para a neblina, pela altura
/// do sol, pela luz global e pela lua. Direções (sol, lua) ficam fora: vêm de `horizontal`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyLook {
    pub zenith: Rgb,
    pub mid: Rgb,
    pub horizon: Rgb,
    /// Faixa baixa do lado do ocaso.
    pub glow: Rgb,
    /// Faixa alta do lado do ocaso e cor do halo.
    pub glow_hi: Rgb,
    /// Força da faixa do ocaso, 0..1.
    pub glow_strength: f32,
    /// Força do halo de espalhamento em volta do sol, 0..1.
    pub halo_strength: f32,
    /// Cor do disco do sol (sem o ganho HDR do shader).
    pub sun_color: Rgb,
    /// Visibilidade do disco do sol, 0..1.
    pub sun_disk: f32,
    /// Visibilidade das estrelas, 0..1: 1 com luz < 0,05; 0 com luz >= 0,15.
    pub star_strength: f32,
    /// Fração iluminada da lua, 0 (nova) a 1 (cheia).
    pub moon_phase: f32,
    /// Visibilidade do disco da lua, 0..1 (noite e lua acima do horizonte).
    pub moon_visibility: f32,
    /// Warm gray of the overcast sky, linear RGB (the shader's `cloud_tint` uniform).
    pub cloud_tint: Rgb,
    /// Cloud cover 0..1 driving the shader's gray veil and the tone of the fog.
    pub cloud_cover: f32,
    /// Theme shadow tint, sRGB, kept for tests and later slices.
    pub shadow_tint: Rgb,
    /// Ambient fill of the shaded faces, sRGB (see `ambient_tint`).
    pub ambient: Rgb,
    /// Sky brightness gain of the clear high-sun sky (the shader's `sky_gain` uniform).
    pub sky_gain: f32,
}

/// Parâmetros do céu para uma altura do sol, luz global 0..1, estado da lua, weight of the
/// afternoon warm bias 0..1 and cloud cover 0..1.
pub fn sky_look(
    sun_alt_deg: f64,
    light: f64,
    moon: &MoonState,
    afternoon: f32,
    cloud_cover: f32,
) -> SkyLook {
    let alt = sun_alt_deg as f32;
    let anchors = style::light_look();
    let colors = look_colors(&anchors, sun_alt_deg, afternoon);
    SkyLook {
        zenith: colors.zenith,
        mid: colors.mid,
        horizon: colors.horizon,
        glow: colors.glow,
        glow_hi: colors.glow_hi,
        // The warm band reaches a little past the golden hour, so a 06:10 / 15:30 sun still
        // paints the horizon on its side (peach at sunrise, coral at sunset) while the top stays
        // blue; it is gone by 26 deg.
        glow_strength: smooth(-10.0, -5.0, alt) * (1.0 - smooth(8.0, 26.0, alt)),
        halo_strength: 0.6 * smooth(-12.0, -4.0, alt) * (1.0 - 0.6 * smooth(8.0, 30.0, alt)),
        sun_color: colors.sun,
        sun_disk: smooth(-1.5, 0.0, alt),
        star_strength: smooth(0.15, 0.05, light as f32),
        moon_phase: moon.illumination as f32,
        moon_visibility: smooth(-1.0, 2.0, moon.position.altitude_deg as f32)
            * (1.0 - smooth(-4.0, 2.0, alt)),
        cloud_tint: cloud_tint(&anchors),
        cloud_cover: cloud_cover.clamp(0.0, 1.0),
        shadow_tint: anchors.shadow_tint,
        ambient: ambient_tint(
            colors.mid,
            anchors.shadow_tint,
            colors.ambient_warmth,
            anchors.fill,
        ),
        sky_gain: sky_gain(sun_alt_deg, cloud_cover.clamp(0.0, 1.0) as f64),
    }
}

/// Cor do céu numa direção, sem estrelas, sol e lua: a mesma conta de `sky()` em
/// `godot/shaders/sky.gdshader`. Serve à cor da neblina na direção do olhar. `sun_dir` e `eye`
/// são unitários, em coordenadas do Godot (y para cima).
pub fn sky_color(look: &SkyLook, sun_dir: [f32; 3], eye: [f32; 3]) -> Rgb {
    let h = eye[1].max(0.0);
    let mut c = lerp3(look.horizon, look.mid, smooth(0.0, 0.25, h));
    c = lerp3(c, look.zenith, smooth(0.1, 0.9, h));
    // Lado do ocaso: o azimute do sol (mesmo abaixo do horizonte) define o lado.
    let sun_h = norm2([sun_dir[0], sun_dir[2]]);
    let eye_h = norm2([eye[0], eye[2]]);
    let side = (sun_h[0] * eye_h[0] + sun_h[1] * eye_h[1]) * 1.25 - 0.25;
    let side = side.clamp(0.0, 1.0);
    let low = 1.0 - smooth(-0.05, 0.45, eye[1]);
    let band = smooth(0.0, 0.08, eye[1]) * (1.0 - smooth(0.1, 0.35, eye[1]));
    c = lerp3(c, look.glow, look.glow_strength * side * low);
    c = lerp3(c, look.glow_hi, look.glow_strength * side * band * 0.8);
    // Calor suave também fora do lado do sol, perto do horizonte (zero do lado oposto).
    let wide = ((sun_h[0] * eye_h[0] + sun_h[1] * eye_h[1]) * 0.5 + 0.5).clamp(0.0, 1.0);
    c = lerp3(c, look.glow, look.glow_strength * wide * low * WARM_HAZE);
    // Halo: espalhamento para a frente em volta do sol.
    let cs = (eye[0] * sun_dir[0] + eye[1] * sun_dir[1] + eye[2] * sun_dir[2]).max(0.0);
    let halo = look.halo_strength * cs.powi(6) * 0.5;
    let c = [0, 1, 2].map(|i| c[i] + look.glow_hi[i] * halo);
    // Cloud veil, identical to the shader's last mix: heavier near the horizon, so the zenith
    // keeps some blue and an overcast noon is a natural gray instead of flat white. The gray is
    // scaled by the sky's own brightness, so a cloudy night stays dark instead of glowing.
    let lift = ((0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]) * CLOUD_LIFT_GAIN).clamp(0.0, 1.0);
    let cloud = look.cloud_tint.map(|v| v * lift);
    let veil = (look.cloud_cover * (0.55 + 0.25 * (1.0 - h))).clamp(0.0, 0.95);
    lerp3(c, cloud, veil).map(|v| v * look.sky_gain)
}

fn norm2(v: [f32; 2]) -> [f32; 2] {
    let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if len < 1e-6 {
        [0.0, 0.0]
    } else {
        [v[0] / len, v[1] / len]
    }
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
    fn cloud_cover_dims_light_monotonically() {
        assert_eq!(cloud_light_factor(0.0), 1.0);
        assert!((cloud_light_factor(1.0) - (1.0 - CLOUD_LIGHT_LOSS)).abs() < 1e-12);
        // Out-of-range covers are clamped, not extrapolated.
        assert_eq!(cloud_light_factor(-0.5), 1.0);
        assert_eq!(cloud_light_factor(2.0), cloud_light_factor(1.0));
        let no_moon = MoonState {
            position: Horizontal {
                azimuth_deg: 0.0,
                altitude_deg: 0.0,
            },
            illumination: 0.0,
            waxing: false,
        };
        let clear = light_level(60.0, &no_moon, cloud_light_factor(0.0));
        let half = light_level(60.0, &no_moon, cloud_light_factor(0.5));
        let full = light_level(60.0, &no_moon, cloud_light_factor(1.0));
        assert!(clear > half && half > full && full > 0.0);
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

    fn luma(c: Rgb) -> f32 {
        0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
    }

    fn no_moon() -> MoonState {
        MoonState {
            position: Horizontal {
                azimuth_deg: 0.0,
                altitude_deg: -10.0,
            },
            illumination: 0.0,
            waxing: false,
        }
    }

    fn saturation(c: Rgb) -> f32 {
        let max = c[0].max(c[1]).max(c[2]);
        let min = c[0].min(c[1]).min(c[2]);
        if max <= 0.0 {
            0.0
        } else {
            (max - min) / max
        }
    }

    #[test]
    fn moment_table_is_documented_and_monotonic() {
        let anchors = crate::style::light_look();
        let table = moment_table(&anchors);
        assert_eq!(table.len(), 7);
        for pair in table.windows(2) {
            assert!(
                pair[0].key_alt_deg < pair[1].key_alt_deg,
                "table out of order at {}",
                pair[1].name
            );
            for (what, a, b) in [
                ("zenith", luma(pair[0].zenith), luma(pair[1].zenith)),
                ("mid", luma(pair[0].mid), luma(pair[1].mid)),
                ("horizon", luma(pair[0].horizon), luma(pair[1].horizon)),
            ] {
                assert!(
                    a <= b + 1e-6,
                    "{what} luma drops from {} to {}",
                    pair[0].name,
                    pair[1].name
                );
            }
        }
        for m in table {
            assert!(!m.name.is_empty());
            assert!(
                m.description.len() > 20,
                "row '{}' has no human description",
                m.name
            );
            for c in [m.zenith, m.mid, m.horizon, m.sun, m.glow, m.glow_hi] {
                assert!(
                    c.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
                    "row '{}' has an invalid color {c:?}",
                    m.name
                );
            }
            assert!((0.0..=1.0).contains(&m.ambient_warmth));
        }
        // Sampling clamps outside the table instead of extrapolating.
        assert_eq!(zenith_tint(120.0), zenith_tint(90.0));
        assert_eq!(zenith_tint(-90.0), zenith_tint(-18.0));
    }

    #[test]
    fn noon_sun_matches_the_theme_anchor_and_is_not_gray() {
        let anchors = crate::style::light_look();
        let sun = sun_tint(80.0);
        for i in 0..3 {
            assert!(
                (sun[i] - anchors.sun_high[i]).abs() <= 0.06,
                "noon sun {sun:?} is far from the theme sun_high {:?}",
                anchors.sun_high
            );
            // #FFF6E8 +-: still a neutral-warm white.
            let target = [1.0, 0.965, 0.910][i];
            assert!(
                (sun[i] - target).abs() <= 0.07,
                "noon sun {sun:?} is far from #FFF6E8"
            );
        }
        assert!(
            saturation(sun) >= 0.10,
            "noon sun is gray, not warm white: {sun:?}"
        );
        assert!(
            sun[0] >= sun[1] && sun[1] >= sun[2],
            "noon sun is not warm: {sun:?}"
        );
    }

    #[test]
    fn ambient_fill_and_shadow_are_blue_violet_not_gray() {
        let anchors = crate::style::light_look();
        let shadow = anchors.shadow_tint;
        assert!(
            shadow[2] > shadow[0] && shadow[2] > shadow[1],
            "theme shadow tint is not blue: {shadow:?}"
        );
        for alt in [-30.0, 17.0, 60.0] {
            let look = sky_look(alt, 0.8, &no_moon(), 0.0, 0.0);
            let fill = look.ambient;
            assert!(
                fill[2] > fill[0] + 0.05 && fill[2] > fill[1] + 0.05,
                "ambient fill at {alt} deg is not blue: {fill:?}"
            );
            // Some red keeps the blue from reading as cyan; the fill is blue-violet, not gray.
            assert!(
                fill[0] > 0.6 * fill[2],
                "ambient fill at {alt} deg is not violet: {fill:?}"
            );
        }
        // Golden hour is the documented exception: long warm shadows, but only at dusk.
        let golden = sky_look(6.0, 0.8, &no_moon(), 1.0, 0.0).ambient;
        assert!(
            golden[0] > golden[2],
            "golden-hour ambient is not warm: {golden:?}"
        );
        let dawn = sky_look(0.0, 0.8, &no_moon(), 0.0, 0.0).ambient;
        let dusk = sky_look(0.0, 0.8, &no_moon(), 1.0, 0.0).ambient;
        assert!(
            dawn[2] > dawn[0],
            "dawn ambient is not a cool shadow: {dawn:?}"
        );
        assert!(
            dusk[0] > dusk[2],
            "dusk ambient is not a warm shadow: {dusk:?}"
        );
    }

    #[test]
    fn sun_energy_keeps_rising_above_six_degrees() {
        let max = 1.45;
        let (e6, e20, e45, e89) = (
            sun_energy(6.0, max),
            sun_energy(20.0, max),
            sun_energy(45.0, max),
            sun_energy(89.0, max),
        );
        assert!(
            e6 < e20 && e20 < e45 && e45 < e89,
            "sun energy is clamped above 6 deg: {e6} {e20} {e45} {e89}"
        );
        assert!((e89 - max).abs() < 0.01, "zenith sun energy {e89} != {max}");
        assert_eq!(sun_energy(-6.0, max), 0.0);
        let mut prev = -1.0;
        let mut alt = -6.0;
        while alt <= 90.0 {
            let energy = sun_energy(alt, 1.0);
            assert!(energy >= prev - 1e-9, "sun energy drops at {alt} deg");
            prev = energy;
            alt += 0.5;
        }
    }

    #[test]
    fn realistic_preset_keeps_the_previous_sky_table_and_cloud_gray() {
        let realistic = crate::style::parse(
            include_str!("../../../../godot/styles/realistic.json"),
            "realistic",
        )
        .expect("realistic.json parses")
        .light;
        let day = srgb_to_linear(realistic.sky_day);
        let pale = srgb_to_linear(realistic.sky_horizon);
        let table = moment_table(&realistic);
        // The item 124 fixes are for the warm lowpoly horizon; the old themes keep the old rows.
        assert_eq!(table[4].mid, lerp3(day, pale, 0.24));
        assert_eq!(table[4].horizon, lerp3(pale, day, 0.08));
        assert_eq!(table[5].horizon, lerp3(pale, [1.0, 0.96, 0.90], 0.17));
        assert_eq!(
            cloud_tint(&realistic),
            lerp3(pale, [0.435, 0.404, 0.352], 0.70)
        );
        assert!(!warm_horizon(&realistic));
        assert!(warm_horizon(&crate::style::light_look()));
    }

    #[test]
    fn overcast_noon_is_a_natural_warm_gray_not_flat_white() {
        let anchors = crate::style::light_look();
        let gray = cloud_tint(&anchors);
        assert!(
            gray.iter().all(|v| (0.0..=1.0).contains(v)),
            "cloud tint out of range: {gray:?}"
        );
        assert!(
            saturation(gray) <= 0.08,
            "cloud tint is too saturated: {gray:?}"
        );
        assert!(gray[0] >= gray[2], "cloud tint is not warm: {gray:?}");

        let sun_dir = [0.0_f32, 0.8, -0.6];
        let clear = sky_look(60.0, 1.0, &no_moon(), 0.0, 0.0);
        let cloudy = sky_look(60.0, 1.0, &no_moon(), 0.0, 0.7);
        let up = [0.0_f32, 1.0, 0.0];
        let clear_top = sky_color(&clear, sun_dir, up);
        let cloudy_top = sky_color(&cloudy, sun_dir, up);
        assert!(
            cloudy_top[0] / cloudy_top[2] > clear_top[0] / clear_top[2] + 0.1,
            "the cloud veil does not warm the zenith: {clear_top:?} -> {cloudy_top:?}"
        );
        // The cloud veil desaturates the zenith; the clear high-sun gain may make it brighter,
        // so brightness is not the gray test here.
        assert!(saturation(cloudy_top) < saturation(clear_top));
        assert!(cloudy_top.iter().all(|v| *v < 1.5));

        // Full overcast at the horizon is gray, not the pale blue of a clear sky.
        let full = sky_look(60.0, 1.0, &no_moon(), 0.0, 1.0);
        let horizon = sky_color(&full, sun_dir, [1.0, 0.0, 0.0]);
        assert!(
            saturation(horizon) < 0.08,
            "full overcast horizon is not gray: {horizon:?}"
        );
    }

    #[test]
    fn weather_haze_is_a_warm_gray_and_keeps_the_night_dark() {
        let noon = sky_look(60.0, 1.0, &no_moon(), 0.0, 0.7);
        let day = haze_color(&noon, false);
        let warm = haze_color(&noon, true);
        // A foggy noon is a mid gray: not the daylight sky itself, and near-neutral without the
        // atmosphere.
        assert!((0.3..0.9).contains(&luma(day)), "haze luma {}", luma(day));
        assert!(saturation(day) <= 0.05, "plain haze is not gray: {day:?}");
        // The lowpoly atmosphere keeps its warm identity: the fog is a warm gray, not a blue one.
        assert!(
            warm[0] > warm[2] + 0.05,
            "atmosphere haze is not warm: {warm:?}"
        );
        assert!(
            saturation(warm) <= 0.25,
            "atmosphere haze too saturated: {warm:?}"
        );
        // A foggy night is as dark as the overcast sky it hides behind: the warm haze is scaled by
        // the horizon's own brightness instead of glowing.
        let night = sky_look(-30.0, 0.0, &no_moon(), 0.0, 0.7);
        let dark = haze_color(&night, true);
        assert!(luma(dark) < 0.3 * luma(warm), "night haze glows: {dark:?}");
        // And the fog follows the sky brightness through the day.
        let dusk = haze_color(&sky_look(0.0, 0.4, &no_moon(), 1.0, 0.7), true);
        assert!(luma(dusk) < luma(warm), "dusk haze {dusk:?}");
    }

    #[test]
    fn afternoon_is_warmer_than_morning_at_the_same_altitude() {
        let morning = sun_tint_at(30.0, 0.0);
        let afternoon = sun_tint_at(30.0, 1.0);
        assert!(
            morning[2] - afternoon[2] > 0.01,
            "afternoon {afternoon:?} is not warmer than morning {morning:?}"
        );
        assert!(
            luma(look_colors(&crate::style::light_look(), 30.0, 1.0).zenith)
                <= luma(zenith_tint(30.0)) + 1e-6
        );
        assert_eq!(afternoon_warmth(-40.0), 0.0);
        assert_eq!(afternoon_warmth(60.0), 1.0);
        assert!((0.0..1.0).contains(&afternoon_warmth(20.0)));
    }

    #[test]
    fn low_sun_has_more_ambient_fill_than_high_sun() {
        let low = ambient_energy(10.0);
        let high = ambient_energy(60.0);
        assert!(
            low > high + 0.1,
            "low-sun fill {low} is not above the high-sun fill {high}"
        );
        assert!(ambient_energy(1.0) > ambient_energy(30.0));
        assert!(ambient_energy(13.0) > 0.9);
        assert!(ambient_energy(-40.0) >= 0.4);
        assert!((ambient_energy(60.0) - AMBIENT_DAY).abs() < 1e-9);
        // Above the low-sun shoulder the fill comes down toward the high-sun value.
        assert!(ambient_energy(10.0) > ambient_energy(20.0));
        assert!(ambient_energy(20.0) > ambient_energy(45.0));
        // It ramps up from the night floor to the low-sun shoulder with no dip.
        let mut prev = -1.0;
        let mut alt = -20.0;
        while alt <= 10.0 {
            let energy = ambient_energy(alt);
            assert!(energy >= prev - 1e-9, "ambient fill drops at {alt} deg");
            prev = energy;
            alt += 0.5;
        }
    }

    #[test]
    fn clear_noon_is_brighter_than_overcast() {
        let clear = scene_key(73.0, 0.0);
        let overcast = scene_key(73.0, 0.7);
        assert!(
            clear > overcast,
            "clear noon key {clear} is not above the overcast key {overcast}"
        );
        // The raw exposure is lower for a clear high sun on purpose (the pale concrete would
        // clip); the scene key above is the regression that matters: the final image must not be
        // darker than the overcast one.
        assert!(day_exposure(73.0, 0.0) < day_exposure(73.0, 0.7));
        assert!(sky_gain(73.0, 0.0) > sky_gain(73.0, 0.7));
        assert_eq!(sky_gain(10.0, 0.0), 1.0);
        assert_eq!(sky_gain(-40.0, 0.0), 1.0);
    }

    #[test]
    fn low_sun_rows_are_not_darker_than_the_first_cut() {
        let table = moment_table(&crate::style::light_look());
        let row = |name: &str| {
            *table
                .iter()
                .find(|m| m.name == name)
                .unwrap_or_else(|| panic!("row '{name}' missing"))
        };
        // First-cut item 121: sunset rows were 0.045/0.15/0.30 (zenith), 0.26/0.25/0.42 (mid) and
        // 0.30/0.34/0.52 (horizon); those made 18:00 mean 53. The tuned rows must stay above them
        // so the sunset does not fall back into a dark twilight.
        let sunset = row("sunrise / sunset");
        assert!(luma(sunset.zenith) >= 0.18);
        assert!(luma(sunset.mid) >= 0.43);
        assert!(luma(sunset.horizon) >= 0.42);
        let golden = row("golden hour");
        assert!(luma(golden.mid) >= 0.48);
        let morning = row("morning");
        assert!(luma(morning.zenith) >= luma(sunset.zenith));
        assert!(luma(morning.horizon) >= 0.7);
    }

    #[test]
    fn sky_shader_mirrors_the_cloud_and_haze_parameters() {
        const SHADER: &str = include_str!("../../../../godot/shaders/sky.gdshader");
        for needle in [
            "uniform vec3 cloud_tint",
            "uniform float cloud_cover",
            "uniform float sky_gain",
            "const float CLOUD_LIFT_GAIN = 2.5;",
            "float veil = clamp(cloud_cover * (0.55 + 0.25 * (1.0 - e.y)), 0.0, 0.95);",
            "col = mix(col, cloud_tint * cloud_lift, veil);",
            "COLOR = col * sky_gain;",
            "const float WARM_HAZE = 0.35;",
        ] {
            assert!(
                SHADER.contains(needle),
                "sky.gdshader is missing {needle:?}: keep it in sync with sky.rs"
            );
        }
    }

    #[test]
    fn sky_colors_are_finite_and_in_unit_range() {
        let moon = MoonState {
            illumination: 0.5,
            ..no_moon()
        };
        let sun_dir = [0.6_f32, 0.2, -0.5];
        let eyes = [[0.0_f32, 1.0, 0.0], [0.0, 0.0, -1.0], [0.6, -0.2, 0.3]];
        let mut alt = -90.0;
        while alt <= 90.0 {
            for light in [0.0, 0.5, 1.0] {
                for cover in [0.0, 0.7, 1.0] {
                    let look = sky_look(alt, light, &moon, 0.0, cover);
                    for c in [
                        look.zenith,
                        look.mid,
                        look.horizon,
                        look.glow,
                        look.glow_hi,
                        look.sun_color,
                        look.cloud_tint,
                        look.shadow_tint,
                        look.ambient,
                    ] {
                        assert!(
                            c.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
                            "alt {alt}: {c:?}"
                        );
                    }
                    for x in [
                        look.glow_strength,
                        look.halo_strength,
                        look.sun_disk,
                        look.star_strength,
                        look.moon_visibility,
                    ] {
                        assert!(x.is_finite() && (0.0..=1.0).contains(&x), "alt {alt}: {x}");
                    }
                    assert!(
                        look.sky_gain.is_finite() && (1.0..=2.0).contains(&look.sky_gain),
                        "alt {alt}: sky gain {}",
                        look.sky_gain
                    );
                    for eye in eyes {
                        let c = sky_color(&look, sun_dir, eye);
                        // The clear high-sun gain can push the sky above 1; ACES handles it, so
                        // only finiteness and a sane ceiling are required here.
                        assert!(
                            c.iter().all(|v| v.is_finite() && (0.0..=2.0).contains(v)),
                            "alt {alt}: {c:?}"
                        );
                    }
                }
            }
            alt += 0.5;
        }
    }

    #[test]
    fn zenith_gets_brighter_with_the_sun() {
        let mut prev = 0.0_f32;
        let mut alt = -18.0;
        while alt <= 30.0 {
            let l = luma(zenith_tint(alt));
            assert!(l >= prev - 1e-6, "zenith luma drops at {alt}: {l} < {prev}");
            prev = l;
            alt += 0.25;
        }
        let mut prev = 0.0_f32;
        let mut alt = -12.0;
        while alt <= 25.0 {
            let l = luma(horizon_tint(alt));
            assert!(
                l >= prev - 1e-6,
                "horizon luma drops at {alt}: {l} < {prev}"
            );
            prev = l;
            alt += 0.25;
        }
    }

    #[test]
    fn night_sky_is_dark_but_not_black() {
        for alt in [-90.0, -30.0, -12.0] {
            let look = sky_look(alt, 0.0, &no_moon(), 0.0, 0.0);
            assert!(luma(look.zenith) > 0.01, "zenith at {alt} too dark");
            assert!(
                luma(look.horizon) > luma(look.zenith),
                "horizon at {alt} not above zenith"
            );
        }
    }

    #[test]
    fn sunset_glows_on_the_sun_side_and_stays_blue_opposite() {
        let alt = -1.0;
        let look = sky_look(alt, 0.5, &no_moon(), 0.0, 0.0);
        // Sol no azimute 250 (oeste-sudoeste), abaixo do horizonte; olhar no mesmo azimute e
        // olhar para o lado oposto (70).
        let sun_dir = [-0.9_f32, -0.02, 0.2];
        let toward = norm2([-1.0, 0.2]);
        let eye_side = [toward[0] * 0.98, 0.05, toward[1] * 0.98];
        let eye_opposite = [-eye_side[0], 0.05, -eye_side[2]];
        let warm = sky_color(&look, sun_dir, eye_side);
        assert!(
            warm[0] > 0.8 && warm[0] > 2.0 * warm[2],
            "sun side not orange: {warm:?}"
        );
        let cool = sky_color(&look, sun_dir, eye_opposite);
        assert!(cool[2] > cool[0], "opposite side not blue: {cool:?}");
        // Topo no ocaso: azul-petróleo (verde e azul acima do vermelho).
        let zen = zenith_tint(alt);
        assert!(
            zen[1] > 2.0 * zen[0] && zen[2] > zen[0],
            "zenith not petrol: {zen:?}"
        );
    }

    #[test]
    fn stars_only_when_light_is_low() {
        let moon = no_moon();
        let look = |light| sky_look(-30.0, light, &moon, 0.0, 0.0);
        assert_eq!(look(0.15).star_strength, 0.0);
        assert_eq!(look(0.6).star_strength, 0.0);
        assert_eq!(look(0.0).star_strength, 1.0);
        let mid = look(0.10).star_strength;
        assert!((0.0..1.0).contains(&mid), "star fade {mid}");
    }
}
