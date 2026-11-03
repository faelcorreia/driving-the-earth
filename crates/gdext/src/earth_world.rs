//! Godot node that loads a baked map from the cache, builds the terrain and roads of
//! the neighborhood, and spawns the car on the road nearest to home (the map center),
//! facing south.
use std::path::PathBuf;

use godot::classes::mesh::{ArrayType, PrimitiveType};
use godot::classes::{
    ArrayMesh, CollisionShape3D, ConcavePolygonShape3D, INode3D, MeshInstance3D, Node3D, PackedScene,
    ProjectSettings, StandardMaterial3D, StaticBody3D,
};
use godot::prelude::*;
use godot::tools::try_load;

use crate::map::{load_map, MeshData};
use crate::spawn::road_spawn;

/// Height of the car above the terrain when spawned, in meters.
const SPAWN_LIFT_M: f32 = 1.0;
const TERRAIN_COLOR: (f32, f32, f32) = (0.42, 0.55, 0.36);
const ROAD_COLOR: (f32, f32, f32) = (0.22, 0.22, 0.24);

/// Loads `<cache_root>/<map_id>` and builds the world under this node.
#[derive(GodotClass)]
#[class(base = Node3D, init)]
pub struct EarthWorld {
    base: Base<Node3D>,
    /// Folder name of the map under the cache root (the id given to tilebake).
    #[init(val = GString::from("casa"))]
    #[export]
    map_id: GString,
    /// Cache root. `res://` paths are resolved to the file system.
    #[init(val = GString::from("res://../cache"))]
    #[export]
    cache_root: GString,
}

#[godot_api]
impl INode3D for EarthWorld {
    fn ready(&mut self) {
        let id = self.map_id.to_string();
        let cache_root = PathBuf::from(
            ProjectSettings::singleton()
                .globalize_path(&self.cache_root)
                .to_string(),
        );
        let map = match load_map(&cache_root, &id) {
            Ok(map) => map,
            Err(e) => {
                godot_error!("cannot load map '{id}' from {}: {e}", cache_root.display());
                return;
            }
        };
        godot_print!("map '{id}' loaded: {} tiles", map.tiles.len());

        let terrain_mat = material(TERRAIN_COLOR);
        let road_mat = material(ROAD_COLOR);
        for tile in &map.tiles {
            let geometry = map.tile_geometry(tile);
            let body = solid_body(&geometry.terrain, &terrain_mat);
            self.base_mut().add_child(&body);
            if !geometry.roads.is_empty() {
                // Roads are solid too, so the wheels rest on them instead of on the terrain below.
                let roads = solid_body(&geometry.roads, &road_mat);
                self.base_mut().add_child(&roads);
            }
        }

        // Spawn on the road nearest to home (the local origin), facing south. Without any
        // road, fall back to home itself facing south.
        let (x, y, yaw) = match road_spawn(&map.tiles) {
            Some(pose) => (pose.x, pose.y, pose.yaw),
            None => {
                godot_warn!("map '{id}' has no road to spawn on, spawning at home");
                (0.0, 0.0, std::f32::consts::PI)
            }
        };
        let Some(height) = map.height_at(x, y) else {
            godot_error!("map '{id}' has no tile at the spawn point, car not spawned");
            return;
        };
        let position = Vector3::new(x as f32, height + SPAWN_LIFT_M, -(y as f32));
        match spawn_car(position, yaw) {
            Some(car) => self.base_mut().add_child(&car),
            None => {
                godot_error!("cannot instantiate res://tr4/tr4.tscn");
            }
        }
    }
}

fn material(color: (f32, f32, f32)) -> Gd<StandardMaterial3D> {
    let mut mat = StandardMaterial3D::new_gd();
    mat.set_albedo(Color::from_rgb(color.0, color.1, color.2));
    mat
}

/// Instantiates the car at `position`, turned by `yaw` about +Y.
fn spawn_car(position: Vector3, yaw: f32) -> Option<Gd<Node3D>> {
    let scene: Gd<PackedScene> = try_load("res://tr4/tr4.tscn").ok()?;
    let mut car = scene.instantiate()?.try_cast::<Node3D>().ok()?;
    car.set_position(position);
    car.set_rotation(Vector3::new(0.0, yaw, 0.0));
    Some(car)
}

/// Visible mesh only.
fn mesh_instance(data: &MeshData, mat: &Gd<StandardMaterial3D>) -> Gd<MeshInstance3D> {
    let mut instance = MeshInstance3D::new_alloc();
    instance.set_mesh(&array_mesh(data));
    instance.set_material_override(&mat.clone().upcast::<godot::classes::Material>());
    instance
}

/// Static body with the visible mesh and a concave collision shape (terrain or roads of a tile).
fn solid_body(data: &MeshData, mat: &Gd<StandardMaterial3D>) -> Gd<StaticBody3D> {
    let mut body = StaticBody3D::new_alloc();
    body.add_child(&mesh_instance(data, mat));

    let points: PackedVector3Array = data.triangle_points().into_iter().map(vec3).collect();
    let mut faces = ConcavePolygonShape3D::new_gd();
    faces.set_faces(&points);
    let mut collider = CollisionShape3D::new_alloc();
    collider.set_shape(&faces);
    body.add_child(&collider);
    body
}

fn vec3(p: [f32; 3]) -> Vector3 {
    Vector3::new(p[0], p[1], p[2])
}

fn array_mesh(data: &MeshData) -> Gd<ArrayMesh> {
    let vertices: PackedVector3Array = data.vertices.iter().map(|&p| vec3(p)).collect();
    let normals: PackedVector3Array = data.normals.iter().map(|&p| vec3(p)).collect();
    let indices: PackedInt32Array = data.indices.iter().map(|&i| i as i32).collect();

    let mut arrays = VarArray::new();
    arrays.resize(ArrayType::MAX.ord() as usize, &Variant::nil());
    arrays.set(ArrayType::VERTEX.ord() as usize, &vertices.to_variant());
    arrays.set(ArrayType::NORMAL.ord() as usize, &normals.to_variant());
    arrays.set(ArrayType::INDEX.ord() as usize, &indices.to_variant());

    let mut mesh = ArrayMesh::new_gd();
    mesh.add_surface_from_arrays(PrimitiveType::TRIANGLES, &arrays);
    mesh
}
