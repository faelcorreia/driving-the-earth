//! Scene nodes and meshes: materials, mesh instances, collision bodies and the car. This is
//! the Godot-facing render layer. May import `geometry` (mesh data), `world` and `environment`.
//! Only `render` and `world` (and `earth_world`, the orchestrator) use the Godot API.

use godot::classes::mesh::{ArrayType, PrimitiveType};
use godot::classes::{
    ArrayMesh, CollisionShape3D, ConcavePolygonShape3D, MeshInstance3D, Node3D, PackedScene,
    StandardMaterial3D, StaticBody3D,
};
use godot::prelude::*;

use crate::geometry::MeshData;

pub(crate) fn material(color: (f32, f32, f32)) -> Gd<StandardMaterial3D> {
    let mut mat = StandardMaterial3D::new_gd();
    mat.set_albedo(Color::from_rgb(color.0, color.1, color.2));
    mat
}

/// Instantiates the car at `position`, turned by `yaw` about +Y.
pub(crate) fn spawn_car(position: Vector3, yaw: f32) -> Option<Gd<Node3D>> {
    let scene: Gd<PackedScene> = try_load("res://tr4/tr4.tscn").ok()?;
    let mut car = scene.instantiate()?.try_cast::<Node3D>().ok()?;
    car.set_position(position);
    car.set_rotation(Vector3::new(0.0, yaw, 0.0));
    Some(car)
}

/// Visible mesh only.
pub(crate) fn mesh_instance(data: &MeshData, mat: &Gd<StandardMaterial3D>) -> Gd<MeshInstance3D> {
    let mut instance = MeshInstance3D::new_alloc();
    instance.set_mesh(&array_mesh(data));
    instance.set_material_override(&mat.clone().upcast::<godot::classes::Material>());
    instance
}

/// Static body with the visible mesh and a concave collision shape (terrain or roads of a tile).
pub(crate) fn solid_body(data: &MeshData, mat: &Gd<StandardMaterial3D>) -> Gd<StaticBody3D> {
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

pub(crate) fn array_mesh(data: &MeshData) -> Gd<ArrayMesh> {
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
