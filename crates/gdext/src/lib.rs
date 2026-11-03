use godot::prelude::*;

mod earth_world;
mod render;
mod environment;
mod geometry;
mod world;
#[cfg(test)]
mod measure;

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
