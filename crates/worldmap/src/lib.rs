//! World map for choosing the home place (F17b). Pure: no Godot types, no file access except
//! the caller passing bytes. The Godot side (`gdext`) reads the asset and draws it.
//!
//! - `projection`: Web Mercator, latitude clamped to +-85 degrees.
//! - `view`: zoom and pan state, screen transforms, which world copies and polylines are visible.
//! - `cities`: which cities show at a zoom, and greedy label placement without overlaps.
//! - `asset`: decoding of `godot/assets/worldmap/world.bin` (written by `scripts/build_worldmap.py`).
//! - `triangulate`: ear clipping of land and water rings, computed once at load.
//! - `tiles`: OpenFreeMap vector tiles (F17b3): tile ids, zoom for a view, MVT decoding (roads, names).
//! - `cull`: bounding boxes and chunks of polylines, so a frame draws only what is on screen.
//! - `ribbon`: road ribbons (triangle strips with miter or bevel joins), built once per tile.
//! - `road_labels`: road names rotated along their lines, spaced and fitted to straight spots.
//! - `labels`: zoom step, label layout cache key and the rule for when a layout is recomputed.
//! - `fetch`: TileJSON template, disk cache of raw `.pbf` tiles (capped) and download with retry.
//! - `home`: the saved home `{lat, lon}`: parse, validate, and write `user://home.json`.
pub mod asset;
pub mod cities;
pub mod cull;
pub mod fetch;
pub mod home;
pub mod labels;
pub mod projection;
pub mod ribbon;
pub mod road_labels;
pub mod tiles;
pub mod triangulate;
pub mod view;
