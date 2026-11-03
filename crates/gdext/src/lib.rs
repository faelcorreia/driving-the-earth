use godot::prelude::*;

mod earth_world;
mod environment;
mod geometry;
#[cfg(test)]
mod measure;
mod render;
mod ui_text;
mod world;

struct DriveEarthExt;

#[gdextension]
unsafe impl ExtensionLibrary for DriveEarthExt {}

/// Test node: confirms that the Rust extension loaded in Godot.
#[derive(GodotClass)]
#[class(base = Node, init)]
struct EarthRoot {
    base: Base<Node>,
}

#[godot_api]
impl INode for EarthRoot {
    fn ready(&mut self) {
        godot_print!("driving_the_earth_ext loaded");
    }
}
