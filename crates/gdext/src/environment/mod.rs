//! Environment layer: the world clock, sun and moon from home's latitude, the light level with its
//! threshold signals and the weather fog. `sky` and `fog` are pure math; `clock` is the Godot node
//! that applies them. May import: `sky`, `fog` and `crate::world` data only. `sky` and `fog` import
//! nothing from Godot nor from `crate::world`.
//! Exception: `clock` (sun light and sky) and `time_panel` (the time panel screen) are Godot-facing.
mod clock;
pub mod fog;
pub mod lamps;
pub mod sky;
mod time_panel;
pub mod time_ui;

pub use clock::EnvironmentClock;
