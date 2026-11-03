//! Environment layer: the world clock, sun and moon from home's latitude, and the light level
//! with its threshold signals. `sky` is pure math; `clock` is the Godot node that applies it.
//! May import: `sky` and `crate::world` data only. `sky` imports nothing from this crate or Godot.
//! Exception: `clock` is the one Godot-facing file here (it drives the sun light and the sky).
mod clock;
pub mod lamps;
pub mod sky;

pub use clock::EnvironmentClock;
