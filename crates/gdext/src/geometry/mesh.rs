//! Triangle meshes in Godot world space, shared by the geometry and the render layer.
//! Pure Rust (no Godot types). Depends on nothing else in this crate.

/// Triangle mesh in Godot world space. `indices` takes three at a time.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MeshData {
    pub vertices: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

impl MeshData {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// The triangles as a flat list of points (three per triangle), for concave collision.
    pub fn triangle_points(&self) -> Vec<[f32; 3]> {
        self.indices.iter().map(|&i| self.vertices[i as usize]).collect()
    }
}

/// Terrain and road meshes of one tile.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TileGeometry {
    pub terrain: MeshData,
    pub roads: MeshData,
}

/// Local `(x, y)` plus height to Godot world space.
pub(super) fn world(x: f64, y: f64, height: f32) -> [f32; 3] {
    [x as f32, height, -(y as f32)]
}

pub(super) fn normalize(v: [f64; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [(v[0] / len) as f32, (v[1] / len) as f32, (v[2] / len) as f32]
}

/// Adds the triangle with its winding fixed so that its front faces up.
/// Godot front faces are clockwise seen from above, i.e. the y component of
/// `(b - a) x (c - a)` is negative.
pub(super) fn push_up_facing(mesh: &mut MeshData, a: u32, b: u32, c: u32) {
    let (pa, pb, pc) = (mesh.vertices[a as usize], mesh.vertices[b as usize], mesh.vertices[c as usize]);
    // y component of (b - a) x (c - a)
    let cross_y = (pb[2] - pa[2]) * (pc[0] - pa[0]) - (pb[0] - pa[0]) * (pc[2] - pa[2]);
    if cross_y <= 0.0 {
        mesh.indices.extend([a, b, c]);
    } else {
        mesh.indices.extend([a, c, b]);
    }
}
