//! Street lamp timing (pure, no Godot). A lamp is lit when the street-lamp threshold has been
//! crossed downwards at game time `on_since_s` and its own delay has passed: the delay is a
//! stable hash of the lamp id, so the lamps of a street light up over a few game minutes instead
//! of all on the same frame. Times are game seconds since the clock started.

use tileformat::units::lamps::LAMP_STAGGER_MAX_MIN;

use super::sky::stagger_delay_min;

/// Delay of a lamp after the threshold crossing, in game seconds (0 to `LAMP_STAGGER_MAX_MIN`
/// minutes, from the id).
pub fn delay_s(id: u64) -> f64 {
    stagger_delay_min(&id.to_string(), LAMP_STAGGER_MAX_MIN) * 60.0
}

/// Whether a lamp is lit at game time `now_s`, given the crossing time `on_since_s` (`None` when
/// the street lamps are off) and the lamp's own delay in seconds.
pub fn is_lit(now_s: f64, on_since_s: Option<f64>, delay: f64) -> bool {
    match on_since_s {
        Some(on) => now_s >= on + delay,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_is_stable_and_within_the_stagger_window() {
        for id in [0u64, 1, 42, u64::MAX, 0x9E37_79B9_7F4A_7C15] {
            let d = delay_s(id);
            assert_eq!(d, delay_s(id));
            assert!((0.0..=LAMP_STAGGER_MAX_MIN as f64 * 60.0).contains(&d), "delay {d} for {id}");
        }
    }

    #[test]
    fn delays_spread_over_the_window() {
        let mut seen = std::collections::BTreeSet::new();
        for index in 0..500u64 {
            seen.insert(delay_s(index * 7919) as u64);
        }
        assert!(seen.len() >= LAMP_STAGGER_MAX_MIN as usize, "only {} distinct delays", seen.len());
    }

    #[test]
    fn lit_only_after_the_crossing_and_the_delay() {
        assert!(!is_lit(1000.0, None, 0.0));
        assert!(!is_lit(1000.0, Some(900.0), 120.0));
        assert!(is_lit(1020.0, Some(900.0), 120.0));
        assert!(is_lit(900.0, Some(900.0), 0.0));
    }
}
