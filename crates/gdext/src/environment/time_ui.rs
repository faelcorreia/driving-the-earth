//! Painel de hora do jogo, lógica pura (sem Godot): lê e formata data e hora locais de Belo
//! Horizonte, escolhe a velocidade, calcula os atalhos (agora, nascer e pôr do sol, meia-noite,
//! lua cheia e lua nova), os astros do dia, e anda pelos campos de data e hora com passo e
//! limites. Textos de UI ficam no `ui_text` (quem desenha o painel pede as chaves).

use super::sky::{self, MoonState};

/// Velocidade acelerada do painel: segundos de jogo por segundo real. Igual ao atalho `\`.
pub const FAST_SPEED: f64 = 60.0;

/// Menor e maior ano escolhíveis no painel.
const YEAR_MIN: i64 = 1900;
const YEAR_MAX: i64 = 2199;

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
    /// Próxima meia-noite local (00:00 do dia seguinte à hora do jogo).
    Midnight,
    /// Próxima lua cheia a partir da hora do jogo.
    FullMoon,
    /// Próxima lua nova a partir da hora do jogo.
    NewMoon,
}

/// Campo do seletor de data e hora, na ordem em que o painel os mostra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Day,
    Month,
    Year,
    Hour,
    Minute,
}

impl Field {
    /// Os campos na ordem do painel.
    pub const ALL: [Field; 5] = [
        Field::Day,
        Field::Month,
        Field::Year,
        Field::Hour,
        Field::Minute,
    ];

    /// Menor e maior valor do campo, dados o mês e o ano escolhidos (o dia depende do mês).
    pub fn bounds(self, year: i64, month: u32) -> (i64, i64) {
        match self {
            Field::Day => (1, i64::from(days_in_month(year, month))),
            Field::Month => (1, 12),
            Field::Year => (YEAR_MIN, YEAR_MAX),
            Field::Hour => (0, 23),
            Field::Minute => (0, 59),
        }
    }

    /// Chave de i18n do nome do campo.
    pub fn key(self) -> &'static str {
        match self {
            Field::Day => "time.day",
            Field::Month => "time.month",
            Field::Year => "time.year",
            Field::Hour => "time.hour",
            Field::Minute => "time.minute",
        }
    }
}

/// Data e hora escolhidas nos campos do painel (dia civil local de Belo Horizonte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Picked {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

impl Picked {
    /// Campos com a data e a hora locais de `unix`.
    pub fn from_unix(unix: f64) -> Picked {
        let (year, month, day) = sky::local_date(unix);
        // A margem evita que 17:30 saia como 17:29 por erro de ponto flutuante.
        let minutes = ((sky::local_hour(unix) * 60.0 + 1e-6).floor() as u32).min(24 * 60 - 1);
        Picked {
            year,
            month,
            day,
            hour: minutes / 60,
            minute: minutes % 60,
        }
    }

    /// Instante Unix da data e hora escolhidas.
    pub fn to_unix(self) -> f64 {
        sky::local_to_unix(
            self.year,
            self.month,
            self.day,
            f64::from(self.hour) + f64::from(self.minute) / 60.0,
        )
    }

    /// Valor atual de um campo.
    pub fn value(self, field: Field) -> i64 {
        match field {
            Field::Day => i64::from(self.day),
            Field::Month => i64::from(self.month),
            Field::Year => self.year,
            Field::Hour => i64::from(self.hour),
            Field::Minute => i64::from(self.minute),
        }
    }

    /// Anda `delta` passos no campo, parando nos limites (sem dar a volta). Depois o dia cai para
    /// o último dia do mês, se o mês ou o ano mudou (31/01 + 1 mês = 28/02; 29/02/2028 + 1 ano
    /// = 28/02/2029).
    pub fn step(self, field: Field, delta: i64) -> Picked {
        let (lo, hi) = field.bounds(self.year, self.month);
        let value = (self.value(field) + delta).clamp(lo, hi);
        let mut next = self;
        match field {
            Field::Day => next.day = value as u32,
            Field::Month => next.month = value as u32,
            Field::Year => next.year = value,
            Field::Hour => next.hour = value as u32,
            Field::Minute => next.minute = value as u32,
        }
        let (_, last_day) = Field::Day.bounds(next.year, next.month);
        next.day = next.day.min(last_day as u32);
        next
    }
}

/// Instante Unix de destino de um atalho. `None` se o sol não nasce ou não se põe em 24 h
/// (regiões polares, em tese: BH não cai aqui) ou se a fase da lua não aparece em 35 dias.
pub fn shortcut_target(shortcut: Shortcut, sim_unix: f64, real_unix: f64) -> Option<f64> {
    match shortcut {
        Shortcut::Now => Some(real_unix),
        Shortcut::Sunrise => sky::next_sunrise(sim_unix),
        Shortcut::Sunset => sky::next_sunset(sim_unix),
        Shortcut::Midnight => Some(next_midnight(sim_unix)),
        Shortcut::FullMoon => next_moon_phase(sim_unix, true),
        Shortcut::NewMoon => next_moon_phase(sim_unix, false),
    }
}

/// Instante da próxima meia-noite local: 00:00 do dia civil seguinte a `sim_unix`.
pub fn next_midnight(sim_unix: f64) -> f64 {
    let (y, m, d) = sky::local_date(sim_unix);
    // A hora 24 de um dia é a meia-noite do seguinte (BRT não tem horário de verão).
    sky::local_to_unix(y, m, d, 24.0)
}

/// Primeiro instante depois de `from_unix`, em até 35 dias, da lua cheia (`full`) ou da nova.
/// A lua cheia é quando a lua passa a minguar (elongação de 180°); a nova, quando passa a
/// crescer (0°). Varredura de 1 h, depois bisseção sobre a mudança de `waxing`.
pub fn next_moon_phase(from_unix: f64, full: bool) -> Option<f64> {
    // Estado de `waxing` depois da fase: a cheia termina em minguante, a nova em crescente.
    let target = !full;
    let mut prev = from_unix;
    let mut prev_waxing = sky::moon(prev).waxing;
    for i in 1..=35 * 24 {
        let t = from_unix + 3600.0 * f64::from(i);
        let waxing = sky::moon(t).waxing;
        if waxing != prev_waxing && waxing == target {
            let (mut lo, mut hi) = (prev, t);
            for _ in 0..30 {
                let mid = 0.5 * (lo + hi);
                if sky::moon(mid).waxing == prev_waxing {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            return Some(0.5 * (lo + hi));
        }
        prev = t;
        prev_waxing = waxing;
    }
    None
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
    fn day_bounds_follow_the_month_and_leap_years() {
        assert_eq!(Field::Day.bounds(2026, 1), (1, 31));
        assert_eq!(Field::Day.bounds(2026, 4), (1, 30));
        assert_eq!(
            Field::Day.bounds(2026, 2),
            (1, 28),
            "2026 is not a leap year"
        );
        assert_eq!(Field::Day.bounds(2028, 2), (1, 29), "2028 is a leap year");
        assert_eq!(
            Field::Day.bounds(1900, 2),
            (1, 28),
            "1900 is not a leap year"
        );
        assert_eq!(Field::Day.bounds(2000, 2), (1, 29), "2000 is a leap year");
        assert_eq!(Field::Month.bounds(2026, 1), (1, 12));
        assert_eq!(Field::Year.bounds(2026, 1), (1900, 2199));
        assert_eq!(Field::Hour.bounds(2026, 1), (0, 23));
        assert_eq!(Field::Minute.bounds(2026, 1), (0, 59));
    }

    #[test]
    fn steps_stop_at_each_field_limit() {
        let p = Picked {
            year: 2026,
            month: 11,
            day: 15,
            hour: 17,
            minute: 30,
        };
        assert_eq!(p.step(Field::Hour, 100).hour, 23);
        assert_eq!(p.step(Field::Hour, -100).hour, 0);
        assert_eq!(p.step(Field::Minute, 100).minute, 59);
        assert_eq!(p.step(Field::Minute, -100).minute, 0);
        assert_eq!(p.step(Field::Month, 20).month, 12);
        assert_eq!(p.step(Field::Month, -20).month, 1);
        assert_eq!(p.step(Field::Year, 1000).year, 2199);
        assert_eq!(p.step(Field::Year, -1000).year, 1900);
        assert_eq!(p.step(Field::Day, 100).day, 30, "November has 30 days");
        assert_eq!(p.step(Field::Day, -100).day, 1);
        assert_eq!(p.step(Field::Hour, 1).hour, 18, "one step is one unit");
        assert_eq!(p.step(Field::Minute, -1).minute, 29);
    }

    #[test]
    fn day_follows_the_month_and_leap_years_when_they_change() {
        let jan_31 = Picked {
            year: 2026,
            month: 1,
            day: 31,
            hour: 0,
            minute: 0,
        };
        let feb = jan_31.step(Field::Month, 1);
        assert_eq!((feb.month, feb.day), (2, 28));

        let mar_31 = Picked { month: 3, ..jan_31 };
        assert_eq!(mar_31.step(Field::Month, -1).day, 28);

        let leap_day = Picked {
            year: 2028,
            month: 2,
            day: 29,
            hour: 0,
            minute: 0,
        };
        assert_eq!(
            leap_day.step(Field::Year, 1).day,
            28,
            "2029 is not a leap year"
        );
        assert_eq!(
            leap_day.step(Field::Year, -4).day,
            29,
            "2024 is a leap year"
        );
        assert_eq!(
            leap_day.step(Field::Day, 1).day,
            29,
            "day stops at the month end"
        );
    }

    #[test]
    fn picked_round_trips_through_unix() {
        for (y, m, d, h, mi) in [
            (2026, 11, 15, 17, 30),
            (2028, 2, 29, 0, 0),
            (2030, 12, 31, 23, 59),
        ] {
            let p = Picked {
                year: y,
                month: m,
                day: d,
                hour: h,
                minute: mi,
            };
            assert_eq!(Picked::from_unix(p.to_unix()), p);
        }
    }

    #[test]
    fn midnight_is_the_start_of_the_next_local_day() {
        let noon = sky::local_to_unix(2026, 12, 31, 12.0);
        let midnight = next_midnight(noon);
        assert_eq!(sky::local_date(midnight), (2027, 1, 1));
        assert_eq!(format_hm(sky::local_hour(midnight)), "00:00");

        let feb = sky::local_to_unix(2027, 2, 28, 23.0);
        assert_eq!(sky::local_date(next_midnight(feb)), (2027, 3, 1));
    }

    #[test]
    fn moon_phase_jumps_land_on_full_and_new_moons() {
        let from = sky::local_to_unix(2026, 11, 15, 12.0);
        for full in [true, false] {
            let t = next_moon_phase(from, full).expect("a phase within 35 days");
            assert!(t > from && t < from + 35.0 * 86400.0);
            let moon = sky::moon(t);
            if full {
                assert!(
                    moon.illumination > 0.99,
                    "full moon at {}",
                    moon.illumination
                );
            } else {
                assert!(
                    moon.illumination < 0.01,
                    "new moon at {}",
                    moon.illumination
                );
            }
            // A fase seguinte da mesma espécie vem um ciclo depois (~29,5 dias).
            let next = next_moon_phase(t + 86400.0, full).expect("next phase");
            let cycle_days = (next - t) / 86400.0;
            assert!(
                (28.0..31.0).contains(&cycle_days),
                "cycle {cycle_days} days"
            );
        }
    }

    #[test]
    fn new_shortcuts_are_after_the_game_time() {
        let from = sky::local_to_unix(2026, 11, 15, 12.0);
        for shortcut in [Shortcut::Midnight, Shortcut::FullMoon, Shortcut::NewMoon] {
            let t = shortcut_target(shortcut, from, 0.0).expect("target");
            assert!(t > from, "{shortcut:?}");
        }
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
