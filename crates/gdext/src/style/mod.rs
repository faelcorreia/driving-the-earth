//! Active visual theme, read from `godot/styles/<id>.json` (item 118).
//!
//! Pure Rust: no Godot types here, so the parser, its defaults and the command-line resolution
//! are unit-tested without the engine. The Godot-facing reader (`EarthWorld`) reads the file and
//! installs the parsed theme; `godot/style.gd` reads the same JSON for the Godot-side consumers.
//!
//! Only the fields that change the look exist. Fonts, spacing and layout are not theme fields.

use std::sync::OnceLock;

use tileformat::{GROUND_CONCRETE, GROUND_PARK, GROUND_SIDEWALK, GROUND_WATER};

/// Theme used when nothing else is chosen.
pub const DEFAULT_ID: &str = "lowpoly";

/// Default bevel, in meters.
pub const DEFAULT_BEVEL_M: f64 = 0.22;
/// Default bevel segments (1..=3).
pub const DEFAULT_BEVEL_SEGMENTS: u8 = 1;
/// Default shadow penumbra (0..=2).
pub const DEFAULT_SHADOW_SOFTNESS: f32 = 1.0;

/// Largest bevel accepted, in meters. Larger values would eat small footprints.
const BEVEL_MAX_M: f64 = 1.0;
/// Largest border width accepted, in pixels.
const BORDER_WIDTH_MAX_PX: i32 = 16;
/// Largest corner radius accepted, in pixels.
const CORNER_RADIUS_MAX_PX: i32 = 64;

/// sRGB color, one `f32` per channel in `0..=1`.
pub type Rgb = [f32; 3];

/// Ground, road, building, lamp and car colors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub grass: Rgb,
    pub cement: Rgb,
    pub sidewalk: Rgb,
    pub park: Rgb,
    pub water: Rgb,
    pub road: Rgb,
    /// Four facade/roof colors, picked per building.
    pub buildings: [Rgb; 4],
    pub lamp: Rgb,
    pub car: Rgb,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            grass: builtin_rgb("#79A65A"),
            cement: builtin_rgb("#B9B4AA"),
            sidewalk: builtin_rgb("#CFC9BC"),
            park: builtin_rgb("#5F8C4B"),
            water: builtin_rgb("#5C93A6"),
            road: builtin_rgb("#3B3F46"),
            buildings: [
                builtin_rgb("#E4D5B7"),
                builtin_rgb("#D78E68"),
                builtin_rgb("#A9C3D0"),
                builtin_rgb("#C7A87D"),
            ],
            lamp: builtin_rgb("#FFC24A"),
            car: builtin_rgb("#D9483B"),
        }
    }
}

/// Look of the 3D world: mesh finish plus the palette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldLook {
    pub textures: bool,
    pub flat_shading: bool,
    pub bevel_m: f64,
    pub bevel_segments: u8,
    pub shadow_softness: f32,
    pub palette: Palette,
}

impl Default for WorldLook {
    fn default() -> Self {
        Self {
            textures: false,
            flat_shading: true,
            bevel_m: DEFAULT_BEVEL_M,
            bevel_segments: DEFAULT_BEVEL_SEGMENTS,
            shadow_softness: DEFAULT_SHADOW_SOFTNESS,
            palette: Palette::default(),
        }
    }
}

/// Anchor colors of the sky and of the sun/moon table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightLook {
    pub sun_high: Rgb,
    pub sun_low: Rgb,
    pub sky_day: Rgb,
    pub sky_horizon: Rgb,
    pub shadow_tint: Rgb,
}

impl Default for LightLook {
    fn default() -> Self {
        Self {
            sun_high: builtin_rgb("#FFF3DA"),
            sun_low: builtin_rgb("#FFB27A"),
            sky_day: builtin_rgb("#78B7E8"),
            sky_horizon: builtin_rgb("#E8F1F5"),
            shadow_tint: builtin_rgb("#8FA6C8"),
        }
    }
}

/// UI colors and the two geometry knobs (border width and corner radius, in pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiLook {
    pub panel: Rgb,
    pub ink: Rgb,
    pub accent: Rgb,
    pub border: Rgb,
    pub border_width: i32,
    pub corner_radius: i32,
}

impl Default for UiLook {
    fn default() -> Self {
        Self {
            panel: builtin_rgb("#F4EFE4"),
            ink: builtin_rgb("#2E2A26"),
            accent: builtin_rgb("#D9744A"),
            border: builtin_rgb("#8C7F6B"),
            border_width: 2,
            corner_radius: 18,
        }
    }
}

/// One complete theme.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub id: String,
    pub world: WorldLook,
    pub light: LightLook,
    pub ui: UiLook,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            id: DEFAULT_ID.to_string(),
            world: WorldLook::default(),
            light: LightLook::default(),
            ui: UiLook::default(),
        }
    }
}

/// Built-in theme used when the file is missing or broken: the `lowpoly` defaults.
pub fn builtin(id: &str) -> Style {
    Style {
        id: id.trim().to_string(),
        ..Style::default()
    }
}

/// Parses one theme. Every missing field keeps the `lowpoly` default; a color that cannot be
/// read also keeps its default. Out-of-range numbers are clamped. `id` is the file id, used when
/// the JSON has no non-empty `"id"`.
pub fn parse(text: &str, id: &str) -> Result<Style, String> {
    let root: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
    let root = root
        .as_object()
        .ok_or_else(|| "theme is not a JSON object".to_string())?;

    let mut style = Style::default();
    if let Some(name) = root
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        style.id = name.to_string();
    } else {
        style.id = id.trim().to_string();
    }

    if let Some(world) = section(root, "world") {
        style.world.textures = flag(world, "textures", style.world.textures);
        style.world.flat_shading = flag(world, "flat_shading", style.world.flat_shading);
        style.world.bevel_m = number(world, "bevel_m", style.world.bevel_m).clamp(0.0, BEVEL_MAX_M);
        style.world.bevel_segments = segments(world, "bevel_segments", style.world.bevel_segments);
        style.world.shadow_softness =
            number(world, "shadow_softness", style.world.shadow_softness as f64).clamp(0.0, 2.0)
                as f32;
        if let Some(palette) = section(world, "palette") {
            style.world.palette = parse_palette(palette, style.world.palette);
        }
    }

    if let Some(light) = section(root, "light") {
        style.light = LightLook {
            sun_high: color(light, "sun_high", style.light.sun_high),
            sun_low: color(light, "sun_low", style.light.sun_low),
            sky_day: color(light, "sky_day", style.light.sky_day),
            sky_horizon: color(light, "sky_horizon", style.light.sky_horizon),
            shadow_tint: color(light, "shadow_tint", style.light.shadow_tint),
        };
    }

    if let Some(ui) = section(root, "ui") {
        style.ui = UiLook {
            panel: color(ui, "panel", style.ui.panel),
            ink: color(ui, "ink", style.ui.ink),
            accent: color(ui, "accent", style.ui.accent),
            border: color(ui, "border", style.ui.border),
            border_width: pixels(
                ui,
                "border_width",
                style.ui.border_width,
                BORDER_WIDTH_MAX_PX,
            ),
            corner_radius: pixels(
                ui,
                "corner_radius",
                style.ui.corner_radius,
                CORNER_RADIUS_MAX_PX,
            ),
        };
    }

    Ok(style)
}

/// Theme id given by `--style <id>` (or `--style=<id>`), if any. `args` are the Godot user args,
/// the ones after `--` on the command line.
pub fn theme_id_from_args<S: AsRef<str>>(args: &[S]) -> Option<String> {
    let mut args = args.iter().map(AsRef::as_ref);
    while let Some(arg) = args.next() {
        if arg == "--style" {
            let id = args.next()?.trim();
            return (!id.is_empty()).then(|| id.to_string());
        }
        if let Some(id) = arg.strip_prefix("--style=") {
            let id = id.trim();
            return (!id.is_empty()).then(|| id.to_string());
        }
    }
    None
}

/// Parses `#RRGGBB` (or `RRGGBB`, or the short `#RGB`) into sRGB channels in `0..=1`.
pub fn parse_hex_color(text: &str) -> Option<Rgb> {
    let text = text.trim();
    let hex = text.strip_prefix('#').unwrap_or(text);
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize, len: usize| -> Option<u8> {
        let part = hex.get(i..i + len)?;
        if len == 1 {
            u8::from_str_radix(part, 16).ok().map(|v| v * 17)
        } else {
            u8::from_str_radix(part, 16).ok()
        }
    };
    let (r, g, b) = match hex.len() {
        6 => (byte(0, 2)?, byte(2, 2)?, byte(4, 2)?),
        3 => (byte(0, 1)?, byte(1, 1)?, byte(2, 1)?),
        _ => return None,
    };
    Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0])
}

/// `#RRGGBB` of a color, for the log and for tests.
pub fn to_hex(color: Rgb) -> String {
    let channel = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02X}{:02X}{:02X}",
        channel(color[0]),
        channel(color[1]),
        channel(color[2])
    )
}

/// Palette of the active theme.
pub fn world_palette() -> Palette {
    active().world.palette
}

/// World finish and palette of the active theme.
// Kept for the slices that build the world look (119 buildings, 120 car).
#[allow(dead_code)]
pub fn world_look() -> WorldLook {
    active().world
}

/// Sky and sun anchors of the active theme.
// Kept for the lighting slice (121).
#[allow(dead_code)]
pub fn light_look() -> LightLook {
    active().light
}

/// UI colors and geometry of the active theme.
// Kept for the UI slice (122).
#[allow(dead_code)]
pub fn ui_look() -> UiLook {
    active().ui
}

/// The four building colors of the active theme.
// Kept for the building slice (119).
#[allow(dead_code)]
pub fn building_rgb() -> [Rgb; 4] {
    active().world.palette.buildings
}

/// Color of a ground class (`GROUND_GRASS` and the other `tileformat` classes).
pub fn ground_rgb(class: u8) -> Rgb {
    let palette = world_palette();
    match class {
        GROUND_CONCRETE => palette.cement,
        GROUND_SIDEWALK => palette.sidewalk,
        GROUND_PARK => palette.park,
        GROUND_WATER => palette.water,
        _ => palette.grass,
    }
}

/// Asphalt color of the active theme.
pub fn road_rgb() -> Rgb {
    world_palette().road
}

/// Installs the theme read at startup. Later calls are ignored, so the first one wins.
pub fn install(style: Style) {
    let _ = ACTIVE.set(style);
}

/// The active theme, or the built-in `lowpoly` defaults when nothing was installed yet.
pub fn active() -> &'static Style {
    ACTIVE.get_or_init(Style::default)
}

static ACTIVE: OnceLock<Style> = OnceLock::new();

fn builtin_rgb(hex: &str) -> Rgb {
    parse_hex_color(hex).expect("built-in theme color is valid")
}

fn section<'a>(
    root: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a serde_json::Map<String, serde_json::Value>> {
    root.get(key)?.as_object()
}

fn flag(object: &serde_json::Map<String, serde_json::Value>, key: &str, default: bool) -> bool {
    object
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(default)
}

fn number(object: &serde_json::Map<String, serde_json::Value>, key: &str, default: f64) -> f64 {
    object
        .get(key)
        .and_then(serde_json::Value::as_f64)
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}

fn segments(object: &serde_json::Map<String, serde_json::Value>, key: &str, default: u8) -> u8 {
    number(object, key, default as f64).round().clamp(1.0, 3.0) as u8
}

fn pixels(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: i32,
    max: i32,
) -> i32 {
    number(object, key, default as f64)
        .round()
        .clamp(0.0, max as f64) as i32
}

fn color(object: &serde_json::Map<String, serde_json::Value>, key: &str, default: Rgb) -> Rgb {
    object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .and_then(parse_hex_color)
        .unwrap_or(default)
}

fn parse_palette(object: &serde_json::Map<String, serde_json::Value>, default: Palette) -> Palette {
    let mut palette = Palette {
        grass: color(object, "grass", default.grass),
        cement: color(object, "cement", default.cement),
        sidewalk: color(object, "sidewalk", default.sidewalk),
        park: color(object, "park", default.park),
        water: color(object, "water", default.water),
        road: color(object, "road", default.road),
        buildings: default.buildings,
        lamp: color(object, "lamp", default.lamp),
        car: color(object, "car", default.car),
    };
    if let Some(buildings) = object
        .get("buildings")
        .and_then(serde_json::Value::as_array)
    {
        for (slot, value) in palette.buildings.iter_mut().zip(buildings) {
            if let Some(rgb) = value.as_str().and_then(parse_hex_color) {
                *slot = rgb;
            }
        }
    }
    palette
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOWPOLY: &str = include_str!("../../../../godot/styles/lowpoly.json");
    const REALISTIC: &str = include_str!("../../../../godot/styles/realistic.json");

    #[test]
    fn both_presets_parse() {
        let lowpoly = parse(LOWPOLY, "lowpoly").expect("lowpoly.json parses");
        assert_eq!(lowpoly.id, "lowpoly");
        assert!(!lowpoly.world.textures);
        assert!(lowpoly.world.flat_shading);
        assert_eq!(lowpoly.world.bevel_m, 0.22);
        assert_eq!(lowpoly.world.bevel_segments, 1);
        assert_eq!(lowpoly.world.palette.buildings.len(), 4);
        assert_eq!(lowpoly.ui.corner_radius, 18);

        let realistic = parse(REALISTIC, "realistic").expect("realistic.json parses");
        assert_eq!(realistic.id, "realistic");
        assert!(!realistic.world.flat_shading);
        assert_eq!(realistic.world.bevel_m, 0.06);
        assert_eq!(realistic.world.bevel_segments, 2);
        // Only the flat shading, the bevel and the palette differ from the default theme.
        assert_eq!(realistic.light, lowpoly.light);
        assert_eq!(realistic.ui, lowpoly.ui);
        assert_ne!(realistic.world.palette, lowpoly.world.palette);
    }

    #[test]
    fn built_in_defaults_equal_the_lowpoly_preset() {
        assert_eq!(
            parse(LOWPOLY, DEFAULT_ID).expect("lowpoly.json parses"),
            Style::default()
        );
        assert_eq!(
            parse(REALISTIC, "realistic")
                .expect("realistic.json parses")
                .world
                .palette
                .buildings
                .len(),
            4
        );
    }

    #[test]
    fn missing_fields_take_defaults() {
        let style = parse(
            r##"{"id":"custom","world":{"palette":{"grass":"#112233"}}}"##,
            "custom",
        )
        .expect("partial theme parses");
        assert_eq!(style.id, "custom");
        assert_eq!(
            style.world.palette.grass,
            [
                0x11 as f32 / 255.0,
                0x22 as f32 / 255.0,
                0x33 as f32 / 255.0
            ]
        );
        assert_eq!(style.world.palette.road, Palette::default().road);
        assert_eq!(style.world.palette.buildings, Palette::default().buildings);
        assert_eq!(style.world.bevel_m, DEFAULT_BEVEL_M);
        assert_eq!(style.world.bevel_segments, DEFAULT_BEVEL_SEGMENTS);
        assert_eq!(style.light, LightLook::default());
        assert_eq!(style.ui, UiLook::default());
    }

    #[test]
    fn broken_json_and_unknown_colors_do_not_panic() {
        assert!(parse("{not json", "lowpoly").is_err());
        assert!(parse("[1, 2, 3]", "lowpoly").is_err());

        let style = parse(
            r##"{"world":{"palette":{"road":"#GGGGGG","grass":42}},"ui":{"panel":"blue"}}"##,
            "broken-color",
        )
        .expect("unknown colors keep the defaults");
        assert_eq!(style.world.palette.road, Palette::default().road);
        assert_eq!(style.world.palette.grass, Palette::default().grass);
        assert_eq!(style.ui.panel, UiLook::default().panel);
    }

    #[test]
    fn command_line_style_picks_the_realistic_preset() {
        let args = ["--map", "casa-10km", "--style", "realistic"];
        let id = theme_id_from_args(&args).expect("--style realistic");
        assert_eq!(id, "realistic");
        let style = parse(REALISTIC, &id).expect("realistic.json parses");
        assert_eq!(style.id, id);
        assert!(!style.world.flat_shading);

        assert_eq!(theme_id_from_args(&["--map", "casa-10km"]), None);
        assert_eq!(theme_id_from_args::<&str>(&[]), None);
        assert_eq!(theme_id_from_args(&["--style"]), None);
        assert_eq!(theme_id_from_args(&["--style", "  "]), None);
        assert_eq!(
            theme_id_from_args(&["--style=realistic"]).as_deref(),
            Some("realistic")
        );
    }

    #[test]
    fn numbers_are_clamped() {
        let high = parse(
            r#"{"world":{"bevel_m":5.0,"bevel_segments":9,"shadow_softness":9.0},
                "ui":{"border_width":99,"corner_radius":999}}"#,
            "high",
        )
        .expect("clamping parses");
        assert_eq!(high.world.bevel_m, BEVEL_MAX_M);
        assert_eq!(high.world.bevel_segments, 3);
        assert_eq!(high.world.shadow_softness, 2.0);
        assert_eq!(high.ui.border_width, BORDER_WIDTH_MAX_PX);
        assert_eq!(high.ui.corner_radius, CORNER_RADIUS_MAX_PX);

        let low = parse(
            r#"{"world":{"bevel_m":-1.0,"bevel_segments":0,"shadow_softness":-2.0},
                "ui":{"border_width":-3,"corner_radius":-30}}"#,
            "low",
        )
        .expect("clamping parses");
        assert_eq!(low.world.bevel_m, 0.0);
        assert_eq!(low.world.bevel_segments, 1);
        assert_eq!(low.world.shadow_softness, 0.0);
        assert_eq!(low.ui.border_width, 0);
        assert_eq!(low.ui.corner_radius, 0);
    }

    #[test]
    fn hex_colors_parse_and_round_trip() {
        assert_eq!(parse_hex_color("#FF8000"), Some([1.0, 128.0 / 255.0, 0.0]));
        assert_eq!(parse_hex_color("ff8000"), Some([1.0, 128.0 / 255.0, 0.0]));
        assert_eq!(
            parse_hex_color("  #f80  "),
            Some([1.0, 0x88 as f32 / 255.0, 0.0])
        );
        assert_eq!(parse_hex_color("#12345"), None);
        assert_eq!(parse_hex_color("#GGGGGG"), None);
        assert_eq!(parse_hex_color(""), None);
        assert_eq!(to_hex([1.0, 128.0 / 255.0, 0.0]), "#FF8000");
    }
}
