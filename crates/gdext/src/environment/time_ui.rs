//! Painel de hora do jogo, lógica pura (sem Godot): lê e formata data e hora locais de Belo
//! Horizonte, escolhe a velocidade, calcula os atalhos (agora, nascer e pôr do sol) e os
//! astros do dia. Textos de UI ficam no `ui_text` (quem desenha o painel pede as chaves).

use super::sky::{self, MoonState};

/// Velocidade acelerada do painel: segundos de jogo por segundo real. Igual ao atalho `\`.
pub const FAST_SPEED: f64 = 60.0;

/// Velocidade do relógio escolhida no painel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speed {
    /// Relógio parado.
    Fixed,
    /// 1 segundo de jogo por segundo real.
    Real,
    /// `FAST_SPEED` segundos de jogo por segundo real.
    Fast,
}

impl Speed {
    /// As opções na ordem em que o painel as mostra.
    pub const ALL: [Speed; 3] = [Speed::Fixed, Speed::Real, Speed::Fast];

    /// Segundos de jogo por segundo real.
    pub fn multiplier(self) -> f64 {
        match self {
            Speed::Fixed => 0.0,
            Speed::Real => 1.0,
            Speed::Fast => FAST_SPEED,
        }
    }

    /// Opção mais próxima de uma velocidade qualquer (ex.: 2x cai em `Real`). Corte geométrico
    /// entre 1x e `FAST_SPEED`.
    pub fn nearest(multiplier: f64) -> Speed {
        if multiplier <= 0.0 {
            Speed::Fixed
        } else if multiplier < (FAST_SPEED).sqrt() {
            Speed::Real
        } else {
            Speed::Fast
        }
    }

    /// Índice da opção no `ALL`, para o seletor do painel.
    pub fn index(self) -> usize {
        Speed::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// Chave de i18n do nome da opção.
    pub fn key(self) -> &'static str {
        match self {
            Speed::Fixed => "time.speed.fixed",
            Speed::Real => "time.speed.real",
            Speed::Fast => "time.speed.fast",
        }
    }
}

/// Atalhos do painel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    /// Hora real do sistema (Belo Horizonte).
    Now,
    /// Próximo nascer do sol a partir da hora do jogo.
    Sunrise,
    /// Próximo pôr do sol a partir da hora do jogo.
    Sunset,
}

/// Instante Unix de destino de um atalho. `None` se o sol não nasce ou não se põe em 24 h
/// (regiões polares, em tese: BH não cai aqui).
pub fn shortcut_target(shortcut: Shortcut, sim_unix: f64, real_unix: f64) -> Option<f64> {
    match shortcut {
        Shortcut::Now => Some(real_unix),
        Shortcut::Sunrise => sky::next_sunrise(sim_unix),
        Shortcut::Sunset => sky::next_sunset(sim_unix),
    }
}

/// Nascer e pôr do sol do dia civil local (BRT) de `sim_unix`, como instantes Unix.
pub fn day_events(sim_unix: f64) -> (Option<f64>, Option<f64>) {
    let (y, m, d) = sky::local_date(sim_unix);
    let midnight = sky::local_to_unix(y, m, d, 0.0);
    (sky::next_sunrise(midnight), sky::next_sunset(midnight))
}

/// Chave de i18n da fase da lua. Quarto crescente e minguante caem em crescente/minguante gibosa
/// ou crescente/minguante pela fração iluminada (0,5 é o quarto).
pub fn moon_phase_key(moon: &MoonState) -> &'static str {
    let f = moon.illumination;
    if f < 0.03 {
        "time.moon.new"
    } else if f > 0.97 {
        "time.moon.full"
    } else if moon.waxing {
        if f < 0.5 {
            "time.moon.waxing_crescent"
        } else {
            "time.moon.waxing_gibbous"
        }
    } else if f < 0.5 {
        "time.moon.waning_crescent"
    } else {
        "time.moon.waning_gibbous"
    }
}

/// Porcentagem iluminada da lua, 0..100.
pub fn moon_percent(moon: &MoonState) -> u32 {
    (moon.illumination * 100.0).round().clamp(0.0, 100.0) as u32
}

/// Hora decimal local (0..24) como "HH:MM".
pub fn format_hm(hour: f64) -> String {
    let minutes = (hour * 60.0).round() as i64;
    let minutes = minutes.rem_euclid(24 * 60);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Data local como "DD/MM/AAAA".
pub fn format_date(year: i64, month: u32, day: u32) -> String {
    format!("{day:02}/{month:02}/{year:04}")
}

/// Hora decimal local de um texto "HH:MM" ou "H:MM" (17:30 vira 17.5). `None` se inválido.
pub fn parse_time(text: &str) -> Option<f64> {
    let (h, m) = text.trim().split_once(':')?;
    let h = parse_digits(h, 2)?;
    let m = parse_digits(m, 2)?;
    if h > 23 || m > 59 {
        return None;
    }
    Some(f64::from(h) + f64::from(m) / 60.0)
}

/// Data local de um texto "DD/MM/AAAA" ou "AAAA-MM-DD". Confere o dia do mês (e ano bissexto).
/// `None` se inválida.
pub fn parse_date(text: &str) -> Option<(i64, u32, u32)> {
    let t = text.trim();
    let (year, month, day) = if t.contains('/') {
        let mut parts = t.split('/');
        let (d, m, y) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() {
            return None;
        }
        (
            parse_digits(y, 4)?,
            parse_digits(m, 2)?,
            parse_digits(d, 2)?,
        )
    } else {
        let mut parts = t.split('-');
        let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() {
            return None;
        }
        (
            parse_digits(y, 4)?,
            parse_digits(m, 2)?,
            parse_digits(d, 2)?,
        )
    };
    let year = i64::from(year);
    if !(1900..=2199).contains(&year) || !(1..=12).contains(&month) {
        return None;
    }
    if day < 1 || day > days_in_month(year, month) {
        return None;
    }
    Some((year, month, day))
}

/// Número de até `max_len` dígitos, sem sinal nem espaço.
fn parse_digits(text: &str, max_len: usize) -> Option<u32> {
    let text = text.trim();
    if text.is_empty() || text.len() > max_len || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_time_accepts_hh_mm_and_rejects_the_rest() {
        assert_eq!(parse_time("17:30"), Some(17.5));
        assert_eq!(parse_time("9:05"), Some(9.0 + 5.0 / 60.0));
        assert_eq!(parse_time(" 08:15 "), Some(8.25));
        assert_eq!(parse_time("00:00"), Some(0.0));
        assert_eq!(parse_time("23:59"), Some(23.0 + 59.0 / 60.0));
        assert_eq!(parse_time("24:00"), None);
        assert_eq!(parse_time("12:60"), None);
        assert_eq!(parse_time("1230"), None);
        assert_eq!(parse_time("+5:00"), None);
        assert_eq!(parse_time("ab:cd"), None);
        assert_eq!(parse_time(""), None);
    }

    #[test]
    fn parse_date_accepts_both_formats_and_checks_the_calendar() {
        assert_eq!(parse_date("15/11/2026"), Some((2026, 11, 15)));
        assert_eq!(parse_date("2026-11-15"), Some((2026, 11, 15)));
        assert_eq!(parse_date("29/02/2028"), Some((2028, 2, 29)));
        assert_eq!(parse_date("29/02/2026"), None, "2026 is not a leap year");
        assert_eq!(parse_date("31/04/2026"), None);
        assert_eq!(parse_date("00/11/2026"), None);
        assert_eq!(parse_date("15/13/2026"), None);
        assert_eq!(parse_date("15/11/1800"), None);
        assert_eq!(parse_date("15/11/2026/1"), None);
        assert_eq!(parse_date("15-11"), None);
        assert_eq!(parse_date(""), None);
    }

    #[test]
    fn local_date_and_hour_follow_brt_across_midnight_utc() {
        let midnight = sky::local_to_unix(2026, 11, 15, 0.0);
        // 02:00 UTC on the 15th is 23:00 on the 14th in Belo Horizonte (UTC-3).
        assert_eq!(sky::local_date(midnight - 3600.0), (2026, 11, 14));
        assert_eq!(sky::local_date(midnight), (2026, 11, 15));
        assert_eq!(format_hm(sky::local_hour(midnight - 3600.0)), "23:00");
    }

    #[test]
    fn date_and_time_round_trip_through_unix() {
        for (y, m, d) in [(2020, 1, 1), (2024, 2, 29), (2026, 11, 15), (2030, 12, 31)] {
            let unix = sky::local_to_unix(y, m, d, 12.0);
            assert_eq!(sky::local_date(unix), (y, m, d));
            assert_eq!(format_date(y, m, d).len(), 10);
        }
        assert_eq!(format_hm(17.5), "17:30");
        assert_eq!(format_hm(23.999), "00:00", "rounds up to midnight");
        assert_eq!(format_hm(-0.5), "23:30");
    }

    #[test]
    fn speed_options_map_to_multipliers_and_back() {
        assert_eq!(Speed::Fixed.multiplier(), 0.0);
        assert_eq!(Speed::Real.multiplier(), 1.0);
        assert_eq!(Speed::Fast.multiplier(), FAST_SPEED);
        for s in Speed::ALL {
            assert_eq!(Speed::nearest(s.multiplier()), s);
            assert_eq!(Speed::ALL[s.index()], s);
        }
        assert_eq!(Speed::nearest(2.0), Speed::Real);
        assert_eq!(Speed::nearest(30.0), Speed::Fast);
        assert_eq!(Speed::nearest(-1.0), Speed::Fixed);
    }

    #[test]
    fn now_shortcut_is_the_real_clock_whatever_the_game_time() {
        let real = 1_800_000_000.0;
        assert_eq!(shortcut_target(Shortcut::Now, 0.0, real), Some(real));
    }

    #[test]
    fn sunrise_and_sunset_shortcuts_land_on_the_horizon_crossing() {
        let from = sky::local_to_unix(2026, 11, 15, 12.0);
        let rise = shortcut_target(Shortcut::Sunrise, from, 0.0).expect("sunrise");
        let set = shortcut_target(Shortcut::Sunset, from, 0.0).expect("sunset");
        assert!(
            rise > from && set > from,
            "next events come after the game time"
        );
        assert!(rise < from + 86400.0 && set < from + 86400.0);
        let thr = sky::SUN_RISE_ALT_DEG;
        assert!((sky::sun(rise).altitude_deg - thr).abs() < 0.05);
        assert!((sky::sun(set).altitude_deg - thr).abs() < 0.05);
        // Sunrise: the sun is going up; sunset: going down.
        assert!(sky::sun(rise + 60.0).altitude_deg > sky::sun(rise).altitude_deg);
        assert!(sky::sun(set + 60.0).altitude_deg < sky::sun(set).altitude_deg);
        assert!(
            set < rise,
            "from noon, tonight's sunset comes before tomorrow's sunrise"
        );
    }

    #[test]
    fn day_events_are_on_the_local_day_and_plausible_for_belo_horizonte() {
        let noon = sky::local_to_unix(2026, 11, 15, 12.0);
        let (rise, set) = day_events(noon);
        let (rise, set) = (rise.expect("sunrise"), set.expect("sunset"));
        assert_eq!(sky::local_date(rise), (2026, 11, 15));
        assert_eq!(sky::local_date(set), (2026, 11, 15));
        assert!(
            (4.0..8.0).contains(&sky::local_hour(rise)),
            "sunrise {}",
            sky::local_hour(rise)
        );
        assert!(
            (16.0..20.0).contains(&sky::local_hour(set)),
            "sunset {}",
            sky::local_hour(set)
        );
    }

    #[test]
    fn moon_phase_keys_follow_illumination_and_direction() {
        let at = |illumination: f64, waxing: bool| MoonState {
            position: sky::Horizontal {
                azimuth_deg: 0.0,
                altitude_deg: 0.0,
            },
            illumination,
            waxing,
        };
        assert_eq!(moon_phase_key(&at(0.0, true)), "time.moon.new");
        assert_eq!(moon_phase_key(&at(1.0, false)), "time.moon.full");
        assert_eq!(moon_phase_key(&at(0.2, true)), "time.moon.waxing_crescent");
        assert_eq!(moon_phase_key(&at(0.8, true)), "time.moon.waxing_gibbous");
        assert_eq!(moon_phase_key(&at(0.2, false)), "time.moon.waning_crescent");
        assert_eq!(moon_phase_key(&at(0.8, false)), "time.moon.waning_gibbous");
        assert_eq!(moon_percent(&at(0.504, true)), 50);
        assert_eq!(moon_percent(&at(1.0, true)), 100);
    }
}
