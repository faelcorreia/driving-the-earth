//! World layer: what is read from the baked cache and streamed in. Holds the map index,
//! tile streaming, terrain LOD and spawn choice. Layer `world` may import `geometry`,
//! `environment` and `tileformat`/`tilebake`; `lod` also uses Godot types.
//! `render` and `earth_world` build nodes from it; `geometry` never imports from here.
pub mod far_lights;
pub mod lod;
pub mod map;
pub mod map2d;
pub mod route;
pub mod spawn;
pub mod streaming;
#[cfg(test)]
mod tests;

pub use map::{open_map, MapIndex};
