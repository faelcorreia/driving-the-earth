"""Low-poly, texture-free car GLB for the game (run inside Blender, not with python3).

    blender -b -P scripts/car_glb_lowpoly.py -- --input CAR.glb --output godot/assets/car/car.glb \
        --ratio 0.2 --length 3.7 --track 1.4 --wheelbase 2.36 --wheel-radius 0.29 [--preview DIR]

What it does, in order:
  1. Imports the car (one or more meshes, joined) and keeps the texture of the first material. The
     texture color of each triangle is sampled at its UV center and becomes a color attribute: the
     grayscale texture is the "baked shading" (dark glass and tires). The output has no texture.
  2. Turns the car to the game frame (front -Z, up +Y) and removes stray spikes: vertices far from
     the mean of their neighbours are dissolved.
  3. Finds the four wheels by geometry. Each tire is a circle in the side view: the grid search
     keeps the circle whose inliers spread over the whole outline and touch the ground (a sill, a
     straight line, does not). The four circles share one radius (the median). The triangles inside
     a tire circle, on the tire's side of the car, are the tire; the rest is the body.
  4. Scales the car so its length equals --length. Optional: --track sets the wheel track by
     scaling the width, and --wheelbase moves the axles by stretching the overhangs (the total
     length stays --length). The origin is the car node origin: x and length centre at zero, y on
     the axle height (the wheel centers).
  5. Decimates each part on its own (--ratio, the fraction of triangles kept), removes the needles
     that decimation leaves (triangles with longest edge squared over area above SLIVER_RATIO, the
     holes are filled), and scales the tires radially to --wheel-radius, if given. The body drops
     by the same amount so its ground contact stays with the tires.
  6. Exports one GLB with the nodes "Body", "Wheel_FL", "Wheel_FR", "Wheel_RL", "Wheel_RR". Each wheel
     mesh is centered on its own axle (the X axis through the node origin), so it spins around X.

Body colors are normalised so the median body face is white (1.0), clamped at 1: the game multiplies
them by the theme body color, so the glass and the tires stay darker than the paint. `--preview DIR`
writes two workbench renders to check the result by eye.

Pinned to Blender 4.5 (the glTF exporter option names are the 4.x ones). The tire fit and the needle
threshold are tuned for the TR4 model: check the printed wheel radius and triangle counts and the
preview when the source changes.
"""

import argparse
import sys

import bmesh
import bpy
import numpy as np
from mathutils import Matrix, Vector

WHEEL_NAMES = {(-1, 1): "Wheel_FL", (1, 1): "Wheel_FR", (-1, -1): "Wheel_RL", (1, -1): "Wheel_RR"}
SPIKE_FACTOR = 3.0  # a vertex this many mean-edge lengths away from its neighbours is a spike
SPIKE_PASSES = 2
SLIVER_RATIO = 40.0  # longest edge squared over triangle area; needles from decimation are far above this


def parse_args() -> argparse.Namespace:
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--input", required=True, help="source GLB (textured car)")
    p.add_argument("--output", required=True, help="GLB to write")
    p.add_argument("--ratio", type=float, required=True, help="decimate ratio per part, 0 < r <= 1")
    p.add_argument("--length", type=float, required=True, help="target car length in meters")
    p.add_argument("--track", type=float, default=0.0,
                   help="wheel track (distance between the left and right wheel centers) in meters, "
                   "reached by scaling the width (0 keeps the model's width)")
    p.add_argument("--wheelbase", type=float, default=0.0,
                   help="distance between the front and rear axles in meters: the overhangs are stretched "
                   "to keep --length (0 keeps the model's proportions)")
    p.add_argument("--wheel-radius", type=float, default=0.0,
                   help="wheel radius in meters: the tires are scaled radially to it (0 keeps the model's)")
    p.add_argument("--preview", default="", help="directory for two preview PNGs (optional)")
    args = p.parse_args(argv)
    if not 0.0 < args.ratio <= 1.0:
        p.error("--ratio must be in (0, 1]")
    if args.length <= 0.0:
        p.error("--length must be positive")
    if args.wheel_radius < 0.0:
        p.error("--wheel-radius must be zero or positive")
    return args


def import_car(path: str) -> tuple[bpy.types.Object, np.ndarray | None]:
    """Imports the GLB, joins its meshes, applies transforms, and returns the object and texture pixels."""
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=path)
    meshes = [o for o in bpy.data.objects if o.type == "MESH"]
    if not meshes:
        raise SystemExit("no mesh in " + path)
    bpy.ops.object.select_all(action="DESELECT")
    for o in meshes:
        o.select_set(True)
    bpy.context.view_layer.objects.active = meshes[0]
    if len(meshes) > 1:
        bpy.ops.object.join()
    ob = bpy.context.view_layer.objects.active
    bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
    bpy.ops.object.modifier_add(type="TRIANGULATE")
    bpy.ops.object.modifier_apply(modifier="Triangulate")

    pixels = None
    if ob.data.materials:
        for node in ob.data.materials[0].node_tree.nodes:
            if node.type == "TEX_IMAGE" and node.image is not None:
                img = node.image
                pixels = (img.size[0], img.size[1], np.array(img.pixels[:], dtype=np.float32).reshape(img.size[1], img.size[0], 4))
                break
    return ob, pixels


def remove_spikes(ob: bpy.types.Object) -> int:
    """Dissolves vertices far from the mean of their neighbours. Returns how many were removed."""
    removed_total = 0
    for _ in range(SPIKE_PASSES):
        bm = bmesh.new()
        bm.from_mesh(ob.data)
        bm.verts.ensure_lookup_table()
        spikes = []
        for v in bm.verts:
            nbrs = [e.other_vert(v) for e in v.link_edges]
            if len(nbrs) < 2:
                continue
            mean = sum((n.co for n in nbrs), Vector()) / len(nbrs)
            mean_edge = sum((e.calc_length() for e in v.link_edges), 0.0) / len(v.link_edges)
            if mean_edge > 0.0 and (mean - v.co).length > SPIKE_FACTOR * mean_edge:
                spikes.append(v)
        if not spikes:
            bm.free()
            break
        bmesh.ops.dissolve_verts(bm, verts=spikes)
        bm.to_mesh(ob.data)
        bm.free()
        removed_total += len(spikes)
    return removed_total


def remove_slivers(obj: bpy.types.Object) -> int:
    """Deletes needle triangles (longest edge squared over area above SLIVER_RATIO) and fills the holes.

    Decimation leaves these where a collapsed edge pulled a corner out: thin spikes that stick out
    of the bumper and the fenders. A well-shaped triangle has a ratio of about 2.3.
    """
    bm = bmesh.new()
    bm.from_mesh(obj.data)
    bm.faces.ensure_lookup_table()
    needles = []
    for f in bm.faces:
        longest = max(e.calc_length() for e in f.edges)
        area = max(f.calc_area(), 1e-12)
        if longest * longest / area > SLIVER_RATIO:
            needles.append(f)
    if needles:
        bmesh.ops.delete(bm, geom=needles, context="FACES")
        loose = [v for v in bm.verts if not v.link_faces]
        bmesh.ops.delete(bm, geom=loose, context="VERTS")
        boundary = [e for e in bm.edges if len(e.link_faces) < 2]
        if boundary:
            bmesh.ops.holes_fill(bm, edges=boundary, sides=4)
            bmesh.ops.triangulate(bm, faces=bm.faces[:])
    bm.to_mesh(obj.data)
    bm.free()
    return len(needles)


def kasa_circle(pts: np.ndarray) -> tuple[np.ndarray, float]:
    """Algebraic least-squares circle through 2D points: returns (center, radius)."""
    a = np.c_[2 * pts[:, 0], 2 * pts[:, 1], np.ones(len(pts))]
    b = (pts ** 2).sum(axis=1)
    cx, cy, c = np.linalg.lstsq(a, b, rcond=None)[0]
    return np.array([cx, cy]), float(np.sqrt(max(c + cx * cx + cy * cy, 1e-12)))


def fit_wheel(co: np.ndarray, side: int, end: int) -> tuple[np.ndarray, float, float]:
    """Fits one tire in the side view (y, z), for the corner at side (x sign) and end (y sign).

    Returns (center (x, y, z), radius, half width along x). The tire touches the ground, so its
    center is one radius above z = 0. The search is a grid over the center along y and the radius,
    scored by how many low outer points of that corner lie on the circle: the body's side panels
    are straight lines and score poorly, the tire outline scores well.
    """
    x_abs = np.abs(co[:, 0])
    z_max = float(co[:, 2].max())
    corner = (np.sign(co[:, 0]) == side) & (x_abs > 0.6 * x_abs.max()) & (np.sign(co[:, 1]) == end) & (co[:, 2] < 0.45 * z_max)
    pts = co[corner]
    if len(pts) < 10:
        raise SystemExit("wheel corner side=%d end=%d has %d vertices" % (side, end, len(pts)))
    yz = pts[:, [1, 2]]
    best = (-1, 0.0, 0.0, None)
    for radius in np.linspace(0.10, 0.22, 97) * z_max:
        for yc in np.linspace(yz[:, 0].min(), yz[:, 0].max(), 120):
            d = np.hypot(yz[:, 0] - yc, yz[:, 1] - radius)
            inl = np.abs(d - radius) < 0.06 * radius
            if inl.sum() < 8:
                continue
            # A tire is a closed outline: its inliers spread over the whole circle. A straight sill
            # crosses the circle at two spots, so it covers few angle bins, whatever the radius.
            angles = np.arctan2(yz[inl, 1] - radius, yz[inl, 0] - yc)
            bins = len(np.unique(((angles + np.pi) / (2.0 * np.pi) * 24).astype(int)))
            # The tire touches the ground: some inlier sits at the bottom of the circle.
            bottom = inl & (np.abs(yz[:, 0] - yc) < 0.3 * radius)
            if bottom.any() and yz[bottom, 1].min() > 0.15 * radius:
                continue
            score = bins * 100000 + int(inl.sum())
            if score > best[0]:
                best = (score, yc, radius, inl)
    score, yc, radius, inl = best
    if score < 12 * 100000:
        raise SystemExit("wheel corner side=%d end=%d: no circle (score %d)" % (side, end, score))
    xs = pts[inl, 0] * side
    x_lo, x_hi = float(np.percentile(xs, 3)), float(np.percentile(xs, 97))
    half_width = (x_hi - x_lo) / 2.0
    x_center = side * (x_lo + x_hi) / 2.0
    return np.array([x_center, yc, radius]), radius, half_width


def remap_wheelbase(ob: bpy.types.Object, wheels: dict, length: float, wheelbase: float) -> None:
    """Piecewise-linear stretch along the length axis (y): the axles move to +-wheelbase/2, the
    overhangs keep their ratio and the total length stays `length`. Wheel fits are moved with it."""
    ys = np.array([v.co.y for v in ob.data.vertices])
    y_front = float(np.mean([wheels[(s, 1)][0][1] for s in (-1, 1)]))
    y_rear = float(np.mean([wheels[(s, -1)][0][1] for s in (-1, 1)]))
    over_front = float(ys.max()) - y_front
    over_rear = y_rear - float(ys.min())
    room = length - wheelbase
    new_front = room * over_front / (over_front + over_rear)
    new_rear = room * over_rear / (over_front + over_rear)
    knots_in = [float(ys.min()), y_rear, y_front, float(ys.max())]
    knots_out = [-wheelbase / 2.0 - new_rear, -wheelbase / 2.0, wheelbase / 2.0, wheelbase / 2.0 + new_front]
    for v in ob.data.vertices:
        v.co.y = float(np.interp(v.co.y, knots_in, knots_out))
    for key, value in list(wheels.items()):
        centre = value[0].copy()
        centre[1] = float(np.interp(centre[1], knots_in, knots_out))
        wheels[key] = (centre, value[1], value[2])
    print("wheelbase %.3f m -> %.3f m (overhangs %.3f / %.3f m)" % (y_front - y_rear, wheelbase, new_front, new_rear))


def assign_parts(ob: bpy.types.Object, wheels: dict) -> tuple[np.ndarray, dict]:
    """Returns the part index of every triangle (0 body, 1..4 wheels) and the wheel fits."""
    bm = bmesh.new()
    bm.from_mesh(ob.data)
    bm.faces.ensure_lookup_table()
    part = np.zeros(len(bm.faces), dtype=np.int32)
    names = list(wheels.keys())
    for f in bm.faces:
        c = f.calc_center_median()
        for i, key in enumerate(names, start=1):
            center, radius, half_width = wheels[key]
            d = np.hypot(c.y - center[1], c.z - center[2])
            if d <= radius * 1.02 and abs(c.x - center[0]) <= half_width * 1.25 + 0.01 and np.sign(c.x) == key[0]:
                part[f.index] = i
                break
    bm.free()
    return part, {names[i - 1]: i for i in range(1, len(names) + 1)}


def split_part(src_mesh: bpy.types.Mesh, part: np.ndarray, index: int, name: str) -> bpy.types.Object:
    bm = bmesh.new()
    bm.from_mesh(src_mesh)
    bm.faces.ensure_lookup_table()
    drop = [f for f in bm.faces if part[f.index] != index]
    bmesh.ops.delete(bm, geom=drop, context="FACES")
    loose = [v for v in bm.verts if not v.link_faces]
    bmesh.ops.delete(bm, geom=loose, context="VERTS")
    mesh = bpy.data.meshes.new(name)
    bm.to_mesh(mesh)
    bm.free()
    obj = bpy.data.objects.new(name, mesh)
    bpy.context.scene.collection.objects.link(obj)
    return obj


def decimate(obj: bpy.types.Object, ratio: float) -> None:
    bpy.context.view_layer.objects.active = obj
    mod = obj.modifiers.new("decimate", "DECIMATE")
    mod.ratio = ratio
    bpy.ops.object.modifier_apply(modifier=mod.name)


def sample_colors(obj: bpy.types.Object, pixels) -> np.ndarray:
    """Texture color of each triangle (sampled at its UV centroid). Grey when there is no texture."""
    me = obj.data
    colors = np.ones((len(me.polygons), 4), dtype=np.float32)
    colors[:, :3] = 0.5
    if pixels is not None and me.uv_layers.active is not None:
        w, h, px = pixels
        uv = me.uv_layers.active.data
        for p in me.polygons:
            u = np.mean([uv[i].uv[0] for i in p.loop_indices])
            v = np.mean([uv[i].uv[1] for i in p.loop_indices])
            x = int(min(max(u, 0.0), 0.9999) * w)
            y = int(min(max(v, 0.0), 0.9999) * h)
            colors[p.index] = px[y, x]
    return colors


def write_colors(obj: bpy.types.Object, colors: np.ndarray, gain: float = 1.0) -> None:
    """Drops the UV layers and the material, and stores the colors as a face color attribute."""
    me = obj.data
    while me.uv_layers:
        me.uv_layers.remove(me.uv_layers[0])
    me.materials.clear()
    # The glTF exporter writes corner (per-loop) colors, not face colors: each loop of a face gets it.
    attr = me.color_attributes.new("Col", "FLOAT_COLOR", "CORNER")
    for p in me.polygons:
        c = colors[p.index]
        rgba = tuple(float(min(x * gain, 1.0)) for x in c[:3]) + (1.0,)
        for loop in p.loop_indices:
            attr.data[loop].color = rgba
        p.use_smooth = False
    me.color_attributes.active_color = attr


def normalise_body(body_colors: np.ndarray) -> float:
    """Scales body colors so the median is 1.0 (clamped to 1). Returns the factor used."""
    lum = body_colors[:, :3].mean(axis=1)
    median = float(np.median(lum))
    return 1.0 / median if median > 1e-6 else 1.0


def render_preview(objects: list, out_dir: str) -> None:
    bpy.context.scene.render.engine = "BLENDER_WORKBENCH"
    shading = bpy.context.scene.display.shading
    shading.light = "STUDIO"
    shading.color_type = "VERTEX"
    corners = [o.matrix_world @ Vector(c) for o in objects for c in o.bound_box]
    lo = Vector((min(v.x for v in corners), min(v.y for v in corners), min(v.z for v in corners)))
    hi = Vector((max(v.x for v in corners), max(v.y for v in corners), max(v.z for v in corners)))
    centre = (lo + hi) / 2.0
    size = max(hi - lo)
    cam_data = bpy.data.cameras.new("preview_cam")
    cam_data.clip_end = 100
    cam = bpy.data.objects.new("preview_cam", cam_data)
    bpy.context.collection.objects.link(cam)
    bpy.context.scene.camera = cam
    bpy.context.scene.render.resolution_x = 900
    bpy.context.scene.render.resolution_y = 600
    for i, (x, y, z) in enumerate([(1, -1, 0.6), (-1, 1, 0.6)]):
        cam.location = centre + Vector((x, y, z)) * size * 1.4
        cam.rotation_euler = (centre - cam.location).to_track_quat("-Z", "Y").to_euler()
        bpy.context.scene.render.filepath = f"{out_dir}/car_preview_{i}.png"
        bpy.ops.render.render(write_still=True)


def export(objects: list, path: str) -> None:
    bpy.ops.object.select_all(action="DESELECT")
    for obj in objects:
        obj.select_set(True)
    bpy.context.view_layer.objects.active = objects[0]
    bpy.ops.export_scene.gltf(
        filepath=path,
        export_format="GLB",
        use_selection=True,
        export_yup=True,
        export_apply=False,
        export_materials="NONE",
        export_vertex_color="ACTIVE",
        export_image_format="NONE",
    )


def main() -> None:
    args = parse_args()
    ob, pixels = import_car(args.input)

    # Game frame: Blender imports the car with its front at -Y and up at +Z. Turning it 180 degrees
    # about Z puts the front at +Y, and the glTF exporter (Z-up to Y-up) then gives front = -Z.
    ob.rotation_euler = (0.0, 0.0, np.pi)
    bpy.context.view_layer.objects.active = ob
    bpy.ops.object.transform_apply(location=False, rotation=True, scale=False)

    spikes = remove_spikes(ob)

    bm = bmesh.new()
    bm.from_mesh(ob.data)
    co = np.array([v.co for v in bm.verts], dtype=np.float64)
    bm.free()
    lo, hi = co.min(axis=0), co.max(axis=0)
    length = float(hi[1] - lo[1])
    scale = args.length / length
    co = co * scale
    # Scaled coordinates: wheel fits happen in the scaled Blender frame (x side, y length, z up).
    bpy.ops.object.select_all(action="DESELECT")
    ob.select_set(True)
    bpy.context.view_layer.objects.active = ob
    ob.scale = (scale, scale, scale)
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)

    wheels = {}
    for side in (-1, 1):
        for end in (-1, 1):
            center, radius, half_width = fit_wheel(co, side, end)
            wheels[(side, end)] = (center, radius, half_width)
    # The four tires are the same part: one radius (the median of the fits) and one fitted y each.
    common_radius = float(np.median([v[1] for v in wheels.values()]))
    # The car is symmetric: both sides of the track get the mean distance from the middle.
    track_half = float(np.mean([abs(v[0][0]) for v in wheels.values()]))
    wheels = {k: (np.array([k[0] * track_half, v[0][1], common_radius]), common_radius, v[2]) for k, v in wheels.items()}

    if args.track > 0.0:
        track = 2.0 * track_half
        fx = args.track / track
        ob.scale = (fx, 1.0, 1.0)
        bpy.context.view_layer.objects.active = ob
        bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
        wheels = {k: (np.array([v[0][0] * fx, v[0][1], v[0][2]]), v[1], v[2] * fx) for k, v in wheels.items()}
        co = np.array([v.co for v in ob.data.vertices], dtype=np.float64)
        print("track %.3f m -> %.3f m (x scale %.4f)" % (track, args.track, fx))

    if args.wheelbase > 0.0:
        remap_wheelbase(ob, wheels, args.length, args.wheelbase)
        co = np.array([v.co for v in ob.data.vertices], dtype=np.float64)

    part, _ = assign_parts(ob, wheels)

    # Axle height: the wheel centers sit one wheel radius above the ground (z = 0 in the source).
    axle_z = float(np.mean([v[0][2] for v in wheels.values()]))
    front_y = np.mean([wheels[(s, 1)][0][1] for s in (-1, 1)])
    rear_y = np.mean([wheels[(s, -1)][0][1] for s in (-1, 1)])
    origin = Vector((0.0, float((front_y + rear_y) / 2.0), axle_z))

    # Split and decimate the parts, then move them all to the car origin.
    src = ob.data
    objects = []
    body = split_part(src, part, 0, "Body")
    decimate(body, args.ratio)
    objects.append(body)
    for key, index in zip(wheels.keys(), range(1, 5)):
        name = WHEEL_NAMES[key]
        wheel = split_part(src, part, index, name)
        decimate(wheel, args.ratio)
        objects.append(wheel)
    spikes += sum(remove_spikes(obj) + remove_slivers(obj) for obj in objects)
    bpy.data.objects.remove(ob, do_unlink=True)

    colors = {obj.name: sample_colors(obj, pixels) for obj in objects}
    body_factor = normalise_body(colors["Body"])
    for obj in objects:
        write_colors(obj, colors[obj.name], body_factor if obj.name == "Body" else 1.0)

    # The tires shrink about their axles to --wheel-radius; the body keeps its ground contact with
    # the tires, so it drops by the same amount as the tire bottoms rise.
    body_drop = (axle_z - args.wheel_radius) if args.wheel_radius > 0.0 else 0.0
    for obj in objects:
        obj.location = -origin + (Vector((0.0, 0.0, body_drop)) if obj.name == "Body" else Vector())
        bpy.context.view_layer.objects.active = obj
    bpy.ops.object.select_all(action="DESELECT")
    for obj in objects:
        obj.select_set(True)
    bpy.ops.object.transform_apply(location=True, rotation=False, scale=False)

    # Each wheel mesh is centered on its own axle: its vertices move to the axle, and the object
    # location (the glTF node translation) carries the axle position on the car. With
    # --wheel-radius the tire is scaled radially about the axle, so the radius is the game's.
    for key in wheels.keys():
        obj = next(o for o in objects if o.name == WHEEL_NAMES[key])
        center = wheels[key][0] - origin
        if args.wheel_radius > 0.0:
            k = args.wheel_radius / wheels[key][1]
            for v in obj.data.vertices:
                v.co.y = (v.co.y - center[1]) * k + center[1]
                v.co.z = (v.co.z - center[2]) * k + center[2]
        obj.data.transform(Matrix.Translation(-Vector(center)))
        obj.location = Vector(center)
    bpy.ops.object.select_all(action="DESELECT")
    for obj in objects:
        obj.select_set(True)

    tris = {o.name: len(o.data.polygons) for o in objects}
    print("spikes removed: %d" % spikes)
    print("length %.3f m -> %.3f m (scale %.4f)" % (length, args.length, scale))
    for key, (center, radius, half_width) in wheels.items():
        print("wheel %s center %s radius %.3f m half width %.3f m" % (WHEEL_NAMES[key], np.round(center - origin, 3), args.wheel_radius or radius, half_width))
    print("triangles: %s total %d" % (tris, sum(tris.values())))
    print("body factor %.3f" % body_factor)
    width = float(co[:, 0].max() - co[:, 0].min())
    print("body width %.3f m" % width)

    if args.preview:
        render_preview(objects, args.preview)
    export(objects, args.output)
    print("wrote", args.output)


if __name__ == "__main__":
    main()
