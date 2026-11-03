//! Street lamp nodes (L1): per tile, three MultiMeshes (poles, heads, light pools) and a fixed
//! pool of real lights moved to the lit lamps nearest the car. Godot-facing; the lists of lamps
//! and their timing are pure (`geometry::lamps`, `environment::lamps`).
use godot::classes::base_material_3d::CullMode;
use godot::classes::light_3d::Param;
use godot::classes::multi_mesh::TransformFormat;
use godot::classes::{
    MultiMesh, MultiMeshInstance3D, Node3D, OmniLight3D, Shader, ShaderMaterial, StandardMaterial3D,
};
use godot::prelude::*;

use super::{array_mesh, material};
use crate::geometry::lamps::{glow_model, head_model, pole_model, LampPost};
use tileformat::units::lamps::{LAMP_ARM_M, LAMP_HEIGHT_M, LAMP_REAL_RANGE_M};

/// Colour of the lamp head and the light pool when lit: sodium yellow.
pub(crate) const LAMP_HEAD_RGB: (f32, f32, f32) = (1.0, 0.72, 0.35);
/// Energy of a lit lamp head (emission) and of a real light.
const HEAD_EMISSION: f32 = 4.0;
const REAL_LIGHT_ENERGY: f32 = 2.0;
/// Real lights in the pool (the export `lamp_real_lights` turns some or all of them off).
pub(crate) const LAMP_POOL: usize = 4;
const POLE_RGB: (f32, f32, f32) = (0.18, 0.18, 0.2);

/// Materials shared by all lamps.
pub(crate) struct LampMaterials {
    pole: Gd<StandardMaterial3D>,
    head: Gd<ShaderMaterial>,
    glow: Gd<ShaderMaterial>,
    pole_mesh: Gd<godot::classes::ArrayMesh>,
    head_mesh: Gd<godot::classes::ArrayMesh>,
    glow_mesh: Gd<godot::classes::ArrayMesh>,
}

impl LampMaterials {
    /// None when the shaders cannot be loaded (no render, or a missing file); the lamps are then
    /// not drawn, the rest of the world is.
    pub(crate) fn new() -> Option<Self> {
        let head_shader: Gd<Shader> = try_load("res://shaders/lamp_head.gdshader").ok()?;
        let glow_shader: Gd<Shader> = try_load("res://shaders/lamp_glow.gdshader").ok()?;
        let mut head = ShaderMaterial::new_gd();
        head.set_shader(&head_shader);
        head.set_shader_parameter(
            "lamp_color",
            &Vector3::new(LAMP_HEAD_RGB.0, LAMP_HEAD_RGB.1, LAMP_HEAD_RGB.2).to_variant(),
        );
        head.set_shader_parameter("lamp_energy", &HEAD_EMISSION.to_variant());
        let mut glow = ShaderMaterial::new_gd();
        glow.set_shader(&glow_shader);
        glow.set_shader_parameter(
            "lamp_color",
            &Vector3::new(LAMP_HEAD_RGB.0, LAMP_HEAD_RGB.1, LAMP_HEAD_RGB.2).to_variant(),
        );
        let mut pole = material(POLE_RGB);
        pole.set_cull_mode(CullMode::DISABLED);
        Some(Self {
            pole,
            head,
            glow,
            pole_mesh: array_mesh(&pole_model()),
            head_mesh: array_mesh(&head_model()),
            glow_mesh: array_mesh(&glow_model()),
        })
    }
}

/// The lamps of one tile as a node with three MultiMeshes (three draw calls). The glow can be
/// hidden (export `lamp_glow_enabled`). Custom data of the head and glow instances: the lamp's
/// own delay in seconds, read by the shaders.
pub(crate) fn lamp_node(
    posts: &[LampPost],
    mats: &LampMaterials,
    glow_visible: bool,
) -> Gd<Node3D> {
    let mut node = Node3D::new_alloc();
    let pole = multimesh(posts, &mats.pole_mesh, false);
    let mut pole_i = MultiMeshInstance3D::new_alloc();
    pole_i.set_multimesh(&pole);
    pole_i.set_material_override(&mats.pole.clone().upcast::<godot::classes::Material>());
    node.add_child(&pole_i);

    let head = multimesh(posts, &mats.head_mesh, true);
    let mut head_i = MultiMeshInstance3D::new_alloc();
    head_i.set_multimesh(&head);
    head_i.set_material_override(&mats.head.clone().upcast::<godot::classes::Material>());
    node.add_child(&head_i);

    let glow = multimesh(posts, &mats.glow_mesh, true);
    let mut glow_i = MultiMeshInstance3D::new_alloc();
    glow_i.set_multimesh(&glow);
    glow_i.set_material_override(&mats.glow.clone().upcast::<godot::classes::Material>());
    glow_i.set_visible(glow_visible);
    node.add_child(&glow_i);
    node
}

fn multimesh(
    posts: &[LampPost],
    mesh: &Gd<godot::classes::ArrayMesh>,
    custom: bool,
) -> Gd<MultiMesh> {
    let mut mm = MultiMesh::new_gd();
    mm.set_transform_format(TransformFormat::TRANSFORM_3D);
    mm.set_use_custom_data(custom);
    mm.set_mesh(mesh);
    mm.set_instance_count(posts.len() as i32);
    for (i, post) in posts.iter().enumerate() {
        let basis = Basis::from_axis_angle(Vector3::UP, post.turn);
        let origin = Vector3::new(post.base[0], post.base[1], post.base[2]);
        mm.set_instance_transform(i as i32, Transform3D::new(basis, origin));
        if custom {
            let delay = crate::environment::lamps::delay_s(post.id) as f32;
            mm.set_instance_custom_data(i as i32, Color::from_rgba(delay, 0.0, 0.0, 0.0));
        }
    }
    mm
}

/// Where the lamp head of a post is, in Godot world space.
pub(crate) fn head_position(post: &LampPost) -> Vector3 {
    let (s, c) = post.turn.sin_cos();
    Vector3::new(
        post.base[0] + c * LAMP_ARM_M as f32,
        post.base[1] + LAMP_HEIGHT_M as f32,
        post.base[2] - s * LAMP_ARM_M as f32,
    )
}

/// The fixed pool of real lights: created once, moved to the lit lamps nearest the car.
pub(crate) struct LampLights {
    lights: Vec<Gd<OmniLight3D>>,
}

impl LampLights {
    pub(crate) fn new() -> Self {
        let lights = (0..LAMP_POOL)
            .map(|_| {
                let mut light = OmniLight3D::new_alloc();
                light.set_color(Color::from_rgb(
                    LAMP_HEAD_RGB.0,
                    LAMP_HEAD_RGB.1,
                    LAMP_HEAD_RGB.2,
                ));
                light.set_param(Param::ENERGY, REAL_LIGHT_ENERGY);
                light.set_param(Param::RANGE, LAMP_REAL_RANGE_M as f32);
                light.set_shadow(false);
                light.set_visible(false);
                light
            })
            .collect();
        Self { lights }
    }

    /// Nodes to add to the scene (the pool is owned by the caller's tree).
    pub(crate) fn nodes(&self) -> Vec<Gd<OmniLight3D>> {
        self.lights.clone()
    }

    /// Puts the pool on `heads` (already the nearest lit lamps, nearest first); `active` caps the
    /// number of lights in use (export `lamp_real_lights`, 0 turns them all off).
    pub(crate) fn place(&mut self, heads: &[Vector3], active: usize) {
        for (i, light) in self.lights.iter_mut().enumerate() {
            match heads.get(i).filter(|_| i < active) {
                Some(&p) => {
                    light.set_position(p);
                    light.set_visible(true);
                }
                None => light.set_visible(false),
            }
        }
    }
}
