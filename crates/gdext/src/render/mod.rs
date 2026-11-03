//! Scene nodes and meshes: materials, mesh instances, collision bodies and the car. This is
//! the Godot-facing render layer. May import `geometry` (mesh data), `world` and `environment`.
//! Only `render` and `world` (and `earth_world`, the orchestrator) use the Godot API.

use godot::classes::mesh::{ArrayType, PrimitiveType};
use godot::classes::{
    ArrayMesh, CollisionShape3D, ConcavePolygonShape3D, Material, MeshInstance3D, Node3D, PackedScene,
    Shader, ShaderMaterial, StandardMaterial3D, StaticBody3D,
};
use godot::obj::Inherits;
use godot::prelude::*;

pub(crate) mod far_lights;
pub(crate) mod lamps;

use crate::geometry::MeshData;

pub(crate) fn material(color: (f32, f32, f32)) -> Gd<StandardMaterial3D> {
    let mut mat = StandardMaterial3D::new_gd();
    mat.set_albedo(Color::from_rgb(color.0, color.1, color.2));
    // Per-vertex colors (when a mesh has them) multiply the albedo; without them the vertex color
    // is white and the look is the albedo, as before.
    mat.set_flag(godot::classes::base_material_3d::Flags::ALBEDO_FROM_VERTEX_COLOR, true);
    mat
}

/// Instantiates the car at `position`, turned by `yaw` about +Y.
pub(crate) fn spawn_car(position: Vector3, yaw: f32) -> Option<Gd<Node3D>> {
    let scene: Gd<PackedScene> = try_load("res://car.tscn").ok()?;
    let mut car = scene.instantiate()?.try_cast::<Node3D>().ok()?;
    car.set_position(position);
    car.set_rotation(Vector3::new(0.0, yaw, 0.0));
    Some(car)
}

/// Material of the buildings: vertex colour, windows by mask in the shader (`building.gdshader`).
/// None when the shader cannot be loaded; the buildings then keep the plain vertex-colour material.
pub(crate) fn building_material() -> Option<Gd<ShaderMaterial>> {
    let shader: Gd<Shader> = try_load("res://shaders/building.gdshader").ok()?;
    let mut mat = ShaderMaterial::new_gd();
    mat.set_shader(&shader);
    Some(mat)
}

/// Visible mesh only.
pub(crate) fn mesh_instance<M: Inherits<Material>>(data: &MeshData, mat: &Gd<M>) -> Gd<MeshInstance3D> {
    let mut instance = MeshInstance3D::new_alloc();
    instance.set_mesh(&array_mesh(data));
    instance.set_material_override(&mat.clone().upcast::<Material>());
    instance
}

/// Static body with the visible mesh and a concave collision shape (terrain, roads or buildings of a tile).
pub(crate) fn solid_body<M: Inherits<Material>>(data: &MeshData, mat: &Gd<M>) -> Gd<StaticBody3D> {
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
    let colors: PackedColorArray = data.colors.iter().map(|c| Color::from_rgba(c[0], c[1], c[2], c[3])).collect();
    let uvs: PackedVector2Array = data.uvs.iter().map(|uv| Vector2::new(uv[0], uv[1])).collect();
    let uv2s: PackedVector2Array = data.uv2.iter().map(|uv| Vector2::new(uv[0], uv[1])).collect();

    let mut arrays = VarArray::new();
    arrays.resize(ArrayType::MAX.ord() as usize, &Variant::nil());
    arrays.set(ArrayType::VERTEX.ord() as usize, &vertices.to_variant());
    arrays.set(ArrayType::NORMAL.ord() as usize, &normals.to_variant());
    if !colors.is_empty() {
        arrays.set(ArrayType::COLOR.ord() as usize, &colors.to_variant());
    }
    if !uvs.is_empty() {
        arrays.set(ArrayType::TEX_UV.ord() as usize, &uvs.to_variant());
    }
    if !uv2s.is_empty() {
        arrays.set(ArrayType::TEX_UV2.ord() as usize, &uv2s.to_variant());
    }
    arrays.set(ArrayType::INDEX.ord() as usize, &indices.to_variant());

    let mut mesh = ArrayMesh::new_gd();
    // Custom 0 holds four floats per vertex (not the default 8-bit RGBA).
    mesh.add_surface_from_arrays(PrimitiveType::TRIANGLES, &arrays);
    mesh
}
