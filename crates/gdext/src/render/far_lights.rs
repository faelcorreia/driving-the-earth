//! Distant lights (L2), Godot side: one MultiMeshInstance3D of billboard quads, drawn with the
//! buffer built by `world::far_lights` and shaded by `shaders/far_lights.gdshader`. One draw call;
//! the shader reads the lamp globals, so the lights come on with the street lamps.
use godot::classes::multi_mesh::TransformFormat;
use godot::classes::{Material, MultiMesh, MultiMeshInstance3D, Shader, ShaderMaterial};
use godot::prelude::*;

use super::array_mesh;
use crate::geometry::MeshData;
use crate::world::far_lights::{handover_swap_m, lit_count, FarLoad};
use crate::world::lod::{FOG_BEGIN_M, FOG_END_M};

/// Default `view_radius_m` of `EarthWorld` (the loaded tiles' radius, in meters). The handover to
/// the real street lamp is at this radius: the distant light fades out over `SWAP_BAND_M` centred
/// on it (closer than that, the real lamp stands there).
const VIEW_RADIUS_M: f64 = 1000.0;
const SWAP_M: f32 = handover_swap_m(VIEW_RADIUS_M) as f32;
const SWAP_BAND_M: f32 = 20.0;
/// Height added to a distant light, in meters, so that the relief of the coarse rings does not
/// hide it.
const LIFT_M: f32 = 4.0;
/// Soft floor on the radius of a light on screen, in pixels: a sub-pixel light is not allowed to
/// flicker by aliasing (its diameter is about 2.5 px at the floor).
const MIN_RADIUS_PX: f32 = 1.25;
/// Angular size of a light, in radians (about 2.5 px at 1080p with a 75 degree vertical fov).
const ANGULAR_SIZE_RAD: f32 = 0.0030;
/// Brightness of a lit light (HDR, unshaded).
const INTENSITY: f32 = 2.5;
/// Ramp of a light coming on, in game seconds.
const RAMP_S: f32 = 30.0;
/// Colours at the two ends of the hue range (sodium yellow, warm white).
const SODIUM_RGB: (f32, f32, f32) = (1.0, 0.72, 0.35);
const WARM_RGB: (f32, f32, f32) = (1.0, 0.9, 0.75);

/// The distant lights node: one MultiMesh with every kept light.
pub(crate) struct DistantLights {
    node: Gd<MultiMeshInstance3D>,
    /// Sorted delays of the lights, to count the lit ones.
    delays: Vec<f32>,
    /// Lit lights at the last update.
    pub(crate) lit: usize,
}

impl DistantLights {
    /// None when the shader cannot be loaded; the lights are then not drawn.
    pub(crate) fn new(load: FarLoad) -> Option<Self> {
        let shader: Gd<Shader> = try_load("res://shaders/far_lights.gdshader").ok()?;
        let mut mat = ShaderMaterial::new_gd();
        mat.set_shader(&shader);
        let rgb = |c: (f32, f32, f32)| Vector3::new(c.0, c.1, c.2).to_variant();
        mat.set_shader_parameter("color_sodium", &rgb(SODIUM_RGB));
        mat.set_shader_parameter("color_warm", &rgb(WARM_RGB));
        mat.set_shader_parameter("fog_begin", &FOG_BEGIN_M.to_variant());
        mat.set_shader_parameter("fog_end", &FOG_END_M.to_variant());
        mat.set_shader_parameter("swap_m", &SWAP_M.to_variant());
        mat.set_shader_parameter("swap_band_m", &SWAP_BAND_M.to_variant());
        mat.set_shader_parameter("lift_m", &LIFT_M.to_variant());
        mat.set_shader_parameter("min_radius_px", &MIN_RADIUS_PX.to_variant());
        mat.set_shader_parameter("angular_size", &ANGULAR_SIZE_RAD.to_variant());
        mat.set_shader_parameter("intensity", &INTENSITY.to_variant());
        mat.set_shader_parameter("ramp_s", &RAMP_S.to_variant());

        let count = load.delays.len();
        let mut mm = MultiMesh::new_gd();
        mm.set_transform_format(TransformFormat::TRANSFORM_3D);
        mm.set_use_custom_data(true);
        mm.set_mesh(&array_mesh(&quad_model()));
        mm.set_instance_count(count as i32);
        mm.set_buffer(&PackedFloat32Array::from(load.buffer.as_slice()));
        // Lights span the whole loaded area; the default bounds (the quad) would cull them all.
        mm.set_custom_aabb(Aabb::new(
            Vector3::new(-40000.0, -4000.0, -40000.0),
            Vector3::new(80000.0, 8000.0, 80000.0),
        ));

        let mut node = MultiMeshInstance3D::new_alloc();
        node.set_multimesh(&mm);
        node.set_material_override(&mat.upcast::<Material>());
        node.set_visible(false);
        Some(Self {
            node,
            delays: load.delays,
            lit: 0,
        })
    }

    /// The node to add under the world node.
    pub(crate) fn node(&self) -> Gd<MultiMeshInstance3D> {
        self.node.clone()
    }

    /// Shows the node only while the street lamps are on (by day it costs nothing), and counts
    /// the lights lit at game time `now_s` (`on_since_s` is when the lamps came on).
    pub(crate) fn update(&mut self, now_s: f64, on_since_s: Option<f64>) -> usize {
        match on_since_s {
            Some(on) => {
                self.node.set_visible(true);
                self.lit = lit_count(&self.delays, now_s - on);
            }
            None => {
                self.node.set_visible(false);
                self.lit = 0;
            }
        }
        self.lit
    }

    pub(crate) fn count(&self) -> usize {
        self.delays.len()
    }
}

/// A unit quad in the XY plane (corners at ±1); the shader turns it into a billboard.
fn quad_model() -> MeshData {
    MeshData {
        vertices: vec![
            [-1.0, -1.0, 0.0],
            [1.0, -1.0, 0.0],
            [1.0, 1.0, 0.0],
            [-1.0, 1.0, 0.0],
        ],
        normals: vec![[0.0, 0.0, 1.0]; 4],
        colors: Vec::new(),
        uvs: Vec::new(),
        uv2: Vec::new(),
        indices: vec![0, 1, 2, 0, 2, 3],
    }
}

/// Resident memory of this process in MB (VmRSS), for the load log.
pub(crate) fn rss_mb() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0)
}
