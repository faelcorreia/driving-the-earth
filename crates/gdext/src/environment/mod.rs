//! Environment layer: the world clock, sun and moon from home's latitude, and the light level
//! with its threshold signals. `sky` is pure math; `clock` is the Godot node that applies it.
//! May import: `sky` and `crate::world` data only. `sky` imports nothing from this crate or Godot.
//! Exception: `clock` (sun light and sky) and `time_panel` (the time panel screen) are Godot-facing.
mod clock;
pub mod lamps;
pub mod sky;
mod time_panel;
pub mod time_ui;

pub use clock::EnvironmentClock;
