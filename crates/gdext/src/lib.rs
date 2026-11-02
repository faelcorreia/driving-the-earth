use godot::prelude::*;

struct DriveEarthExt;

#[gdextension]
unsafe impl ExtensionLibrary for DriveEarthExt {}

/// Nó de teste: confirma que a extensão Rust carregou no Godot.
#[derive(GodotClass)]
#[class(base = Node, init)]
struct EarthRoot {
    base: Base<Node>,
}

#[godot_api]
impl INode for EarthRoot {
    fn ready(&mut self) {
        godot_print!("driving_the_earth_ext carregada");
    }
}
