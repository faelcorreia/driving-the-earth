//! World map for choosing the home place (F17b). Pure: no Godot types, no file access except
//! the caller passing bytes. The Godot side (`gdext`) reads the asset and draws it.
//!
//! - `projection`: Web Mercator, latitude clamped to +-85 degrees.
//! - `view`: zoom and pan state, screen transforms, which world copies and polylines are visible.
//! - `cities`: which cities show at a zoom, and greedy label placement without overlaps.
//! - `asset`: decoding of `godot/assets/worldmap/world.bin` (written by `scripts/build_worldmap.py`).
//! - `home`: the saved home `{lat, lon}`: parse, validate, and write `user://home.json`.
pub mod asset;
pub mod cities;
pub mod home;
pub mod projection;
pub mod view;
