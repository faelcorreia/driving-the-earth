//! Weather fog, first slice (backlog 65): fog driven by the cloud cover of item 25. Pure math, no
//! Godot.
//!
//! The scene fog is `FogMode::DEPTH` (`world::lod::configure_view`), where `fog_density` is
//! ignored and the visible falloff comes from the two depth distances and the curve (item 124
//! measured it): the weather fog therefore moves the distances. The `fog_density` variable is the
//! weather intensity, 0 (clear air) to 1 (thick fog), and it is multiplied by the cloud cover and
//! shaped by `DENSITY_GAMMA`, so a clear sky never fogs and an overcast one does. The sky share of
//! the fog and the global light follow the same number, so visibility falls and contrast flattens
//! as it rises. At density 0 the clear-weather values are handed back unchanged and the item 124
//! lowpoly atmosphere is untouched.

/// Perceptual curve of the slider: half of it is already a strong haze, so the visibility falls
/// where a player notices it instead of only at the far hills. The depth fog, the fog color, the
/// sky share and the light all use the same shaped number.
const DENSITY_GAMMA: f32 = 0.5;

/// Visibility (distance where `VISIBILITY_AMOUNT` of the fog sits) at full weather fog, in meters:
/// the far hills are gone, the next blocks are half dissolved and the near street still reads.
pub const FULL_VISIBILITY_M: f32 = 450.0;
/// Distance where the fog starts at full strength: 2% of the visibility, so the near street keeps
/// its contrast.
const FULL_BEGIN_M: f32 = 0.02 * FULL_VISIBILITY_M;
/// Depth fog curve at full weather fog: close to linear, so the fog builds up along the whole reach
/// instead of only past the middle of the clear-weather curve (2).
pub const FULL_CURVE: f32 = 1.1;
/// Share of the sky the fog covers at full strength: the sky turns into the fog color, so the
/// horizon and the far terrain meet. A little of the sky is left, so it is not a flat wall.
pub const SKY_AFFECT_FULL: f32 = 0.9;

/// Fog amount that counts as "the world is gone", for the visibility number in the log.
pub const VISIBILITY_AMOUNT: f32 = 0.95;

/// Shares of the global light, of the direct sun and of the ambient fill the fog changes at full
/// strength: the fog scatters the sun away and lifts the fill, so contrast falls instead of the
/// whole scene turning into the fog color.
pub const LIGHT_LOSS: f32 = 0.20;
pub const SUN_LOSS: f32 = 0.45;
pub const AMBIENT_GAIN: f32 = 0.45;

/// The slider values of the debug shortcut, in order.
const DEBUG_STEPS: [f64; 3] = [0.0, 0.5, 1.0];

/// Effective weather fog strength, 0..1: the `fog_density` slider times the cloud cover, so a
/// clear sky never fogs, shaped by `DENSITY_GAMMA` so the visibility falls across the whole
/// slider. Inputs outside 0..1 are clamped.
pub fn effective(density: f64, cloud_cover: f64) -> f32 {
    let raw = (density.clamp(0.0, 1.0) * cloud_cover.clamp(0.0, 1.0)) as f32;
    raw.powf(DENSITY_GAMMA)
}

/// Depth fog distances and curve: what `FogMode::DEPTH` reads to fade the world into the fog.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FogDepth {
    /// Distance where the fog starts, in meters.
    pub begin_m: f32,
    /// Distance where the fog is opaque, in meters.
    pub end_m: f32,
    /// Falloff exponent between the two distances (1 is linear).
    pub curve: f32,
}

impl Default for FogDepth {
    /// Only a starting value for the clock, which replaces it from the theme in `ready` and every
    /// frame after: it is never the fog of a real scene.
    fn default() -> Self {
        FogDepth {
            begin_m: 0.0,
            end_m: 0.0,
            curve: 1.0,
        }
    }
}

impl FogDepth {
    /// The weather fog depths at `strength` (0..1, see `effective`), starting from the
    /// clear-weather `clear` values. The visibility interpolates geometrically, because it is
    /// judged in the log of the distance: a thin fog already cuts the far view (the hills) before
    /// it reaches the street. `strength` 0 hands the theme values back untouched.
    pub fn faded(clear: FogDepth, strength: f32) -> FogDepth {
        let t = strength.clamp(0.0, 1.0);
        if t <= 0.0 {
            return clear;
        }
        let curve = clear.curve + (FULL_CURVE - clear.curve) * t;
        let visibility = geometric(clear.visibility_m(), FULL_VISIBILITY_M, t);
        let begin_m = geometric(clear.begin_m, FULL_BEGIN_M, t);
        FogDepth {
            begin_m,
            end_m: begin_m + (visibility - begin_m) / VISIBILITY_AMOUNT.powf(1.0 / curve),
            curve,
        }
    }

    /// Fog amount 0..1 at `distance_m`: the share of the fog color over the surface, the same
    /// falloff the scene shaders apply. Used by `distance_at` and the fog tests.
    #[allow(dead_code)] // the frame path only needs `visibility_m`
    pub fn amount(&self, distance_m: f32) -> f32 {
        let span = (self.end_m - self.begin_m).max(1e-3);
        ((distance_m - self.begin_m) / span)
            .clamp(0.0, 1.0)
            .powf(self.curve)
    }

    /// Distance where the fog reaches `amount` (0..1), in meters.
    pub fn distance_at(&self, amount: f32) -> f32 {
        let t = amount.clamp(0.0, 1.0).powf(1.0 / self.curve.max(1e-3));
        self.begin_m + (self.end_m - self.begin_m) * t
    }

    /// Distance where the world is gone (`VISIBILITY_AMOUNT` of fog), in meters: the visibility
    /// reported in the log and in the handback.
    pub fn visibility_m(&self) -> f32 {
        self.distance_at(VISIBILITY_AMOUNT)
    }
}

/// Geometric interpolation of a distance: `t` 0 keeps `a`, 1 gives `b`. Distances are scaled in
/// log space, where visibility lives.
fn geometric(a: f32, b: f32, t: f32) -> f32 {
    if t <= 0.0 {
        return a;
    }
    if t >= 1.0 {
        return b;
    }
    if a <= 0.0 || b <= 0.0 {
        return a + (b - a) * t;
    }
    (a.ln() * (1.0 - t) + b.ln() * t).exp()
}

/// Share of the global light left at `strength`: 1 is clear air.
pub fn light_factor(strength: f32) -> f32 {
    1.0 - LIGHT_LOSS * strength.clamp(0.0, 1.0)
}

/// Share of the direct sun left at `strength`: the fog scatters it, so shadows flatten.
pub fn sun_factor(strength: f32) -> f32 {
    1.0 - SUN_LOSS * strength.clamp(0.0, 1.0)
}

/// Multiplier of the ambient fill at `strength`: the scattered sun lightens the shaded faces.
pub fn ambient_factor(strength: f32) -> f32 {
    1.0 + AMBIENT_GAIN * strength.clamp(0.0, 1.0)
}

/// Next slider value of the debug shortcut: 0 -> 0.5 -> 1 -> 0. Any other value (set by a
/// variable) steps to the next one above it, so 0.3 goes to 0.5.
pub fn next_step(density: f64) -> f64 {
    for step in DEBUG_STEPS {
        if density < step - 1e-9 {
            return step;
        }
    }
    0.0
}

/// Weather fog density from `--fog-density <0..1>` (or `--fog-density=<0..1>`) in the Godot user
/// args, the same shape as `style::theme_id_from_args`. `None` when absent or not a number; a
/// number outside 0..1 is clamped. The captures use it: there is no fog UI yet.
pub fn density_from_args<S: AsRef<str>>(args: &[S]) -> Option<f64> {
    let mut args = args.iter().map(AsRef::as_ref);
    while let Some(arg) = args.next() {
        let value = if arg == "--fog-density" {
            args.next()?
        } else if let Some(value) = arg.strip_prefix("--fog-density=") {
            value
        } else {
            continue;
        };
        return value.trim().parse::<f64>().ok().map(|v| v.clamp(0.0, 1.0));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The item 124 atmosphere reach, the clear-weather baseline of the lowpoly theme.
    const ATMOSPHERE: FogDepth = FogDepth {
        begin_m: 3000.0,
        end_m: 9000.0,
        curve: 2.0,
    };

    #[test]
    fn density_follows_the_cloud_cover() {
        assert_eq!(effective(0.0, 1.0), 0.0);
        assert_eq!(effective(1.0, 0.0), 0.0);
        assert_eq!(effective(1.0, 1.0), 1.0);
        assert!((effective(1.0, 0.7) - 0.7f32.sqrt()).abs() < 1e-6);
        assert!((effective(0.5, 1.0) - 0.5f32.sqrt()).abs() < 1e-6);
        // Out-of-range inputs are clamped, not extrapolated.
        assert_eq!(effective(2.0, 1.0), 1.0);
        assert_eq!(effective(-1.0, 1.0), 0.0);
        assert_eq!(effective(1.0, 2.0), 1.0);
    }

    #[test]
    fn clear_air_keeps_the_theme_depths() {
        assert_eq!(FogDepth::faded(ATMOSPHERE, 0.0), ATMOSPHERE);
        // The clear-weather visibility of the atmosphere, for the log line.
        let vis = ATMOSPHERE.visibility_m();
        assert!((8500.0..9000.0).contains(&vis), "clear visibility {vis}");
        assert_eq!(ATMOSPHERE.amount(3000.0), 0.0);
        assert_eq!(ATMOSPHERE.amount(9000.0), 1.0);
        assert_eq!(ATMOSPHERE.amount(0.0), 0.0);
        assert_eq!(ATMOSPHERE.amount(20000.0), 1.0);
    }

    #[test]
    fn visibility_falls_monotonically_with_the_strength() {
        let mut prev_vis = f32::MAX;
        let mut prev_begin = f32::MAX;
        let mut prev_end = f32::MAX;
        let mut prev_curve = f32::MAX;
        let steps = 100;
        for i in 0..=steps {
            let s = i as f32 / steps as f32;
            let depth = FogDepth::faded(ATMOSPHERE, s);
            let vis = depth.visibility_m();
            assert!(vis < prev_vis, "visibility grows at strength {s}: {vis}");
            assert!(depth.begin_m < prev_begin, "begin grows at strength {s}");
            assert!(depth.end_m < prev_end, "end grows at strength {s}");
            assert!(depth.curve < prev_curve, "curve grows at strength {s}");
            prev_vis = vis;
            prev_begin = depth.begin_m;
            prev_end = depth.end_m;
            prev_curve = depth.curve;
        }
        // The slider positions of a capture, at full cloud cover: the haze is already thick at
        // half and the full setting hides the far hills.
        let half = FogDepth::faded(ATMOSPHERE, effective(0.5, 1.0));
        assert!(
            (900.0..1300.0).contains(&half.visibility_m()),
            "half-slider visibility {}",
            half.visibility_m()
        );
        let full = FogDepth::faded(ATMOSPHERE, effective(1.0, 1.0));
        assert!(
            (FULL_VISIBILITY_M - 1.0..FULL_VISIBILITY_M + 1.0).contains(&full.visibility_m()),
            "full visibility {}",
            full.visibility_m()
        );
        // At full strength a 300 m block is more than half dissolved and the near street still
        // reads (less than 15% at 60 m).
        assert!(
            full.amount(300.0) > 0.5,
            "300 m amount {}",
            full.amount(300.0)
        );
        assert!(
            full.amount(60.0) < 0.15,
            "60 m amount {}",
            full.amount(60.0)
        );
        // A thin haze already cuts the far view: the hills at 3 km lose half their contrast while
        // the street a kilometer out is still clear.
        let thin = FogDepth::faded(ATMOSPHERE, 0.25);
        assert!(
            thin.amount(3000.0) > 0.4,
            "thin fog at 3 km {}",
            thin.amount(3000.0)
        );
        assert!(
            thin.amount(1000.0) < 0.1,
            "thin fog at 1 km {}",
            thin.amount(1000.0)
        );
    }

    #[test]
    fn depth_amount_and_distance_are_inverse() {
        for s in [0.0, 0.2, 0.5, 0.8, 1.0] {
            let depth = FogDepth::faded(ATMOSPHERE, s);
            for amount in [0.1, 0.5, 0.95] {
                let distance = depth.distance_at(amount);
                assert!(
                    (depth.amount(distance) - amount).abs() < 1e-4,
                    "strength {s}, amount {amount}: {}",
                    depth.amount(distance)
                );
                assert!(distance.is_finite() && distance > 0.0);
            }
        }
    }

    #[test]
    fn light_dims_and_the_fill_lifts_with_the_strength() {
        assert_eq!(light_factor(0.0), 1.0);
        assert_eq!(sun_factor(0.0), 1.0);
        assert_eq!(ambient_factor(0.0), 1.0);
        assert!((light_factor(1.0) - (1.0 - LIGHT_LOSS)).abs() < 1e-6);
        assert!(light_factor(1.0) > sun_factor(1.0));
        assert!(ambient_factor(1.0) > 1.0);
        let mut prev = 1.0;
        for i in 0..=10 {
            let s = i as f32 / 10.0;
            assert!(light_factor(s) <= prev + 1e-6, "light rises at {s}");
            assert!(sun_factor(s) >= 0.0 && sun_factor(s) <= prev + 1e-6);
            prev = light_factor(s);
        }
        // Clamped outside 0..1.
        assert_eq!(light_factor(2.0), light_factor(1.0));
        assert_eq!(ambient_factor(-1.0), ambient_factor(0.0));
    }

    #[test]
    fn debug_shortcut_cycles_zero_half_and_full() {
        assert_eq!(next_step(0.0), 0.5);
        assert_eq!(next_step(0.5), 1.0);
        assert_eq!(next_step(1.0), 0.0);
        // A value set by a variable steps to the next one above it.
        assert_eq!(next_step(0.3), 0.5);
        assert_eq!(next_step(0.75), 1.0);
        assert_eq!(next_step(-1.0), 0.0);
    }

    #[test]
    fn capture_argument_reads_the_density() {
        assert_eq!(density_from_args(&["--fog-density", "0.5"]), Some(0.5));
        assert_eq!(
            density_from_args(&["--map", "casa-10km", "--fog-density=1"]),
            Some(1.0)
        );
        assert_eq!(density_from_args(&["--fog-density", "0"]), Some(0.0));
        // Out of range is clamped; a missing value or a non-number is ignored.
        assert_eq!(density_from_args(&["--fog-density", "2"]), Some(1.0));
        assert_eq!(density_from_args(&["--fog-density", "abc"]), None);
        assert_eq!(density_from_args(&["--fog-density"]), None);
        assert_eq!(density_from_args::<&str>(&["--map", "casa-10km"]), None);
        assert_eq!(density_from_args::<&str>(&[]), None);
    }
}
