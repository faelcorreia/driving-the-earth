extends RefCounted
## Procedural low-poly car (backlog 120): body, cabin, bumpers, glass, lamps and wheels.
##
## No GLB and no texture: every piece is a mesh built once at ready from a side profile extruded
## across X. The profile corners are rounded and the side edges are chamfered; `flat_shading`
## switches the normals between per-face (faceted) and averaged (smooth), and `bevel_m` sets how
## big the chamfer and the corner radius are. The theme bevel is sized for the world (buildings),
## so the car scales it down: a 1.5 m car cannot use a 0.22 m chamfer.
##
## Only the visible meshes and their colors are touched. The node names, transforms, collision
## shapes and wheel properties stay as they are in `car.tscn`.

## Lower body half width, m. Kept inside the 1.55 m collision box so the wheels stay proud of it.
const BODY_HALF_WIDTH := 0.755
## Wheel disk parameters (the `Mesh` child of each VehicleWheel3D is built around +Y).
const WHEEL_SIDES := 12
const HUB_SIDES := 10
const WHEEL_RADIUS := 0.29
const WHEEL_WIDTH := 0.20
const HUB_RADIUS := 0.16
const HUB_WIDTH := 0.216
## Glass width as a fraction of the cabin half width.
const GLASS_WIDTH_FACTOR := 0.78
## Glass/trim offset above the shell surface, m (avoids z-fighting).
const SURFACE_OFFSET := 0.004

## Side profile of the lower body in (z, y), front at -Z. The collisions box is y -0.29..0.31.
const BODY_PROFILE := [
	Vector2(-1.86, -0.23),  # front bumper lower lip
	Vector2(-1.86, 0.10),   # front fascia top
	Vector2(-1.62, 0.27),   # hood leading edge
	Vector2(-0.74, 0.31),   # windshield base
	Vector2(1.56, 0.31),    # rear body top
	Vector2(1.82, 0.20),    # hatch shoulder
	Vector2(1.86, -0.08),   # rear fascia
	Vector2(1.62, -0.29),   # rear underbody
	Vector2(-1.50, -0.29),  # front underbody
]

## The cabin shell sinks this far into the body, so the two meshes never share a plane.
const CABIN_BASE_SINK_M := 0.03

## Theme values resolved once, so the mesh build reads the Style autoload only here.
class Look:
	var style_id := "lowpoly"
	var flat := true
	var bevel := 0.055
	var corner := 0.10
	var segments := 1
	var body := Color(0.85, 0.28, 0.23)
	var trim := Color(0.18, 0.19, 0.21)
	var underbody := Color(0.12, 0.13, 0.14)
	var glass := Color(0.10, 0.16, 0.22)
	var tire := Color(0.08, 0.08, 0.09)
	var hub := Color(0.80, 0.78, 0.74)
	var lamp := Color(1.0, 0.82, 0.55)
	var plate := Color(0.88, 0.87, 0.82)

## Accumulates triangles with a per-face color and a smoothing group, then commits an ArrayMesh.
## Flat: one normal per triangle. Smooth: normals averaged per group+position, so the chamfer
## facets melt into the body without smoothing across separate parts.
class MeshBuilder:
	var _faces: Array = []
	var _smooth := false

	func _init(smooth: bool) -> void:
		_smooth = smooth

	## Adds a triangle. `outward` is the side that must face the viewer; the winding is fixed and
	## the shading normal is `outward`, as Godot fronts a triangle clockwise seen from outside.
	func tri(a: Vector3, b: Vector3, c: Vector3, color: Color, group: int, outward: Vector3) -> void:
		var cross := (b - a).cross(c - a)
		if cross.length_squared() < 1e-12:
			return
		if cross.dot(outward) > 0.0:
			var swap := b
			b = c
			c = swap
		_faces.append([a, b, c, color, group, outward.normalized()])

	func quad(a: Vector3, b: Vector3, c: Vector3, d: Vector3, color: Color, group: int, outward: Vector3) -> void:
		tri(a, b, c, color, group, outward)
		tri(a, c, d, color, group, outward)

	func tri_count() -> int:
		return _faces.size()

	func commit() -> ArrayMesh:
		var st := SurfaceTool.new()
		st.begin(Mesh.PRIMITIVE_TRIANGLES)
		if _smooth:
			var sums := {}
			for face in _faces:
				for vertex in [face[0], face[1], face[2]]:
					var key := "%d|%.4f|%.4f|%.4f" % [face[4], vertex.x, vertex.y, vertex.z]
					sums[key] = (sums.get(key, Vector3.ZERO) as Vector3) + (face[5] as Vector3)
			for face in _faces:
				for vertex in [face[0], face[1], face[2]]:
					var key := "%d|%.4f|%.4f|%.4f" % [face[4], vertex.x, vertex.y, vertex.z]
					var normal: Vector3 = (sums[key] as Vector3).normalized()
					st.set_color(face[3])
					st.set_normal(normal)
					st.add_vertex(vertex)
		else:
			for face in _faces:
				for vertex in [face[0], face[1], face[2]]:
					st.set_color(face[3])
					st.set_normal(face[5])
					st.add_vertex(vertex)
		return st.commit()


## Builds every visible mesh on `car` and returns the triangle counts and the headlamp material.
func apply(car: Node3D, cabin_profile: PackedVector2Array, cabin_half_width: float) -> Dictionary:
	var look := _read_look()

	var body_builder := MeshBuilder.new(look.flat)
	_build_body(body_builder, look)
	var body_mesh := body_builder.commit()

	var cabin_builder := MeshBuilder.new(look.flat)
	_build_cabin(cabin_builder, look, cabin_profile, cabin_half_width)
	var cabin_mesh := cabin_builder.commit()

	var glass_builder := MeshBuilder.new(false)
	_build_glass(glass_builder, look, cabin_profile, cabin_half_width)
	var glass_mesh := glass_builder.commit()

	var wheel_builder := MeshBuilder.new(look.flat)
	_build_wheel(wheel_builder, look)
	var wheel_mesh := wheel_builder.commit()

	var tail_builder := MeshBuilder.new(false)
	_build_tail_lamp(tail_builder)
	var tail_mesh := tail_builder.commit()

	var headlamp_builder := MeshBuilder.new(false)
	_build_headlamps(headlamp_builder)
	var headlamp_mesh := headlamp_builder.commit()

	var body_mat := _body_material()
	var wheel_mat := _wheel_material()
	var glass_mat := _glass_material(look)
	var headlamp_mat := _headlamp_material(look)

	var body_node := car.get_node("BodyMesh") as MeshInstance3D
	body_node.mesh = body_mesh
	body_node.material_override = body_mat

	var cabin_node := car.get_node("CabinMesh") as MeshInstance3D
	cabin_node.mesh = cabin_mesh
	cabin_node.material_override = body_mat

	var glass_node := car.get_node("GlassMesh") as MeshInstance3D
	glass_node.mesh = glass_mesh
	glass_node.material_override = glass_mat

	for name in ["FrontLeft", "FrontRight", "RearLeft", "RearRight"]:
		var wheel := car.get_node(name) as VehicleWheel3D
		var mesh_node := wheel.get_node("Mesh") as MeshInstance3D
		mesh_node.mesh = wheel_mesh
		mesh_node.material_override = wheel_mat

	# Tail materials (brake light) are owned by car.gd; only the panel mesh changes here.
	for name in ["TailLeft", "TailRight"]:
		(car.get_node(name) as MeshInstance3D).mesh = tail_mesh

	var headlamp_node := car.get_node("HeadlampMesh") as MeshInstance3D
	headlamp_node.mesh = headlamp_mesh
	headlamp_node.material_override = headlamp_mat

	var wheels_tris := 4 * wheel_builder.tri_count()
	var lamps_tris := 2 * tail_builder.tri_count() + headlamp_builder.tri_count()
	var body_tris := body_builder.tri_count()
	var cabin_tris := cabin_builder.tri_count()
	return {
		"triangles": body_tris + cabin_tris + glass_builder.tri_count() + wheels_tris + lamps_tris,
		"body": body_tris,
		"cabin": cabin_tris,
		"wheels": wheels_tris,
		"lamps": lamps_tris,
		"headlamp_material": headlamp_mat,
		"style_id": look.style_id,
		"flat": look.flat,
		"bevel": look.bevel,
	}


## Reads the active theme: finish knobs plus the car, lamp, road and sidewalk colors.
func _read_look() -> Look:
	var look := Look.new()
	var tree := Engine.get_main_loop() as SceneTree
	var style: Node = tree.root.get_node_or_null("Style") if tree != null else null
	if style == null:
		return look
	look.style_id = str(style.call("id"))
	look.flat = bool(style.call("flag", "world", "flat_shading", true))
	var bevel_m := float(style.call("number", "world", "bevel_m", 0.22))
	look.segments = clampi(int(round(float(style.call("number", "world", "bevel_segments", 1.0)))), 1, 3)
	# Theme bevels are in world meters (buildings use 0.06..0.22 m). A 1.5 m car uses a fraction.
	look.bevel = clampf(bevel_m * 0.25, 0.012, 0.09)
	look.corner = clampf(bevel_m * 0.50, 0.02, 0.11)

	var body: Color = style.call("color", "world.palette", "car", look.body)
	var road: Color = style.call("color", "world.palette", "road", Color(0.23, 0.25, 0.27))
	var sidewalk: Color = style.call("color", "world.palette", "sidewalk", Color(0.81, 0.79, 0.74))
	var lamp: Color = style.call("color", "world.palette", "lamp", Color(1.0, 0.76, 0.29))
	look.body = body
	look.trim = road.darkened(0.35)
	look.underbody = road.darkened(0.55)
	look.glass = road.darkened(0.30).lerp(Color(0.10, 0.15, 0.21), 0.55)
	look.tire = road.darkened(0.62)
	look.hub = sidewalk.lerp(Color.WHITE, 0.25)
	look.lamp = lamp.lerp(Color.WHITE, 0.25)
	look.plate = sidewalk.lerp(Color.WHITE, 0.20)
	return look


# --- body ---------------------------------------------------------------------------------------

func _build_body(builder: MeshBuilder, look: Look) -> void:
	var profile := _round_profile(PackedVector2Array(BODY_PROFILE), look.corner, look.segments + 1)
	_add_prism(builder, profile, BODY_HALF_WIDTH, look.bevel, look.segments, look.body, 0)

	# Bumpers wrap the fascia and stand slightly proud of the body.
	_add_box(builder, Vector3(0.0, -0.10, -1.875), Vector3(1.46, 0.15, 0.10), look.trim, 1)
	_add_box(builder, Vector3(0.0, -0.10, 1.875), Vector3(1.46, 0.15, 0.10), look.trim, 1)
	# Grille and lower intake, dark like the bumpers.
	_add_box(builder, Vector3(0.0, -0.02, -1.872), Vector3(0.62, 0.09, 0.012), look.underbody, 1)
	_add_box(builder, Vector3(0.0, -0.18, -1.874), Vector3(0.92, 0.07, 0.012), look.underbody, 1)
	# Front and rear plates, and the hatch handle under the rear window.
	_add_box(builder, Vector3(0.0, -0.19, -1.932), Vector3(0.30, 0.075, 0.012), look.plate, 1)
	_add_box(builder, Vector3(0.0, -0.10, 1.932), Vector3(0.30, 0.075, 0.012), look.plate, 1)
	_add_box(builder, Vector3(0.0, 0.47, 1.858), Vector3(0.34, 0.030, 0.020), look.trim, 1)

	# Hood detail: two thin crease strips laid on the hood surface (no floating plate).
	var hood_a := Vector2(-1.62, 0.27)
	var hood_b := Vector2(-0.74, 0.31)
	var hood_n := _outward_2d(hood_a, hood_b)
	var ha := hood_a.lerp(hood_b, 0.18) + hood_n * 0.003
	var hb := hood_a.lerp(hood_b, 0.82) + hood_n * 0.003
	for crease_value in [-0.30, 0.30]:
		var crease_x: float = crease_value
		builder.quad(
			Vector3(crease_x - 0.012, ha.y, ha.x), Vector3(crease_x + 0.012, ha.y, ha.x),
			Vector3(crease_x + 0.012, hb.y, hb.x), Vector3(crease_x - 0.012, hb.y, hb.x),
			look.body.darkened(0.18), 1, Vector3(0.0, hood_n.y, hood_n.x))

	# Door seams and handles on both sides.
	for side_value in [-1.0, 1.0]:
		var side: float = side_value
		var x: float = side * (BODY_HALF_WIDTH + SURFACE_OFFSET)
		for z_value in [0.10, 0.78]:
			var z: float = z_value
			builder.quad(
				Vector3(x, -0.22, z - 0.006), Vector3(x, -0.22, z + 0.006),
				Vector3(x, 0.24, z + 0.006), Vector3(x, 0.24, z - 0.006),
				look.trim, 2, Vector3(side, 0.0, 0.0))
		_add_box(builder, Vector3(side * (BODY_HALF_WIDTH + 0.018), 0.155, 0.22),
			Vector3(0.016, 0.032, 0.12), look.trim, 2)

	# Side mirrors: a dark head on a short stalk at the base of the A-pillar.
	for side_value in [-1.0, 1.0]:
		var side: float = side_value
		_add_box(builder, Vector3(side * (BODY_HALF_WIDTH + 0.052), 0.350, -0.60),
			Vector3(0.095, 0.070, 0.100), look.trim, 3)
		_add_box(builder, Vector3(side * (BODY_HALF_WIDTH + 0.008), 0.305, -0.60),
			Vector3(0.050, 0.035, 0.040), look.trim, 3)


# --- cabin and glass ----------------------------------------------------------------------------

## The cabin shell comes from the same profile as the collision prism, sunk into the body.
func _build_cabin(builder: MeshBuilder, look: Look, cabin_profile: PackedVector2Array, half_width: float) -> void:
	var base_y := INF
	for point in cabin_profile:
		base_y = minf(base_y, point.y)
	var shell := PackedVector2Array()
	for point in cabin_profile:
		shell.append(Vector2(point.x, point.y - CABIN_BASE_SINK_M if is_equal_approx(point.y, base_y) else point.y))
	var profile := _round_profile(shell, look.corner * 0.9, look.segments + 1)
	_add_prism(builder, profile, half_width, look.bevel * 0.8, look.segments, look.body, 0)


func _build_glass(builder: MeshBuilder, look: Look, cabin_profile: PackedVector2Array, cabin_half_width: float) -> void:
	var glass_width := cabin_half_width * GLASS_WIDTH_FACTOR
	# Windshield and rear window follow the first and third faces of the collision profile.
	_add_glass_panel(builder, cabin_profile[0], cabin_profile[1], 0.12, 0.92, glass_width, look.glass)
	_add_glass_panel(builder, cabin_profile[2], cabin_profile[3], 0.10, 0.92, glass_width, look.glass)

	# Side windows on the flat side caps: front and rear, split by the B-pillar.
	for side_value in [-1.0, 1.0]:
		var side: float = side_value
		var x: float = side * (cabin_half_width + SURFACE_OFFSET)
		var outward := Vector3(side, 0.0, 0.0)
		builder.quad(
			Vector3(x, 0.49, -0.30), Vector3(x, 0.49, 0.52),
			Vector3(x, 0.855, 0.52), Vector3(x, 0.80, -0.30),
			look.glass, 0, outward)
		builder.quad(
			Vector3(x, 0.49, 0.62), Vector3(x, 0.49, 1.38),
			Vector3(x, 0.78, 1.38), Vector3(x, 0.855, 0.62),
			look.glass, 0, outward)


func _add_glass_panel(builder: MeshBuilder, a: Vector2, b: Vector2, t0: float, t1: float, half_width: float, color: Color) -> void:
	var normal := _outward_2d(a, b)
	var p0 := a.lerp(b, t0) + normal * SURFACE_OFFSET
	var p1 := a.lerp(b, t1) + normal * SURFACE_OFFSET
	builder.quad(
		Vector3(-half_width, p0.y, p0.x), Vector3(half_width, p0.y, p0.x),
		Vector3(half_width, p1.y, p1.x), Vector3(-half_width, p1.y, p1.x),
		color, 0, Vector3(0.0, normal.y, normal.x))


# --- lamps --------------------------------------------------------------------------------------

## Tail lamp panel: a small lowpoly box centered on the node origin (car.gd owns the material).
func _build_tail_lamp(builder: MeshBuilder) -> void:
	_add_box(builder, Vector3.ZERO, Vector3(0.38, 0.09, 0.035), Color.WHITE, 0)


## Headlamp lenses: two quads on the front fascia, one per side, facing -Z.
func _build_headlamps(builder: MeshBuilder) -> void:
	for side_value in [-1.0, 1.0]:
		var side: float = side_value
		var x0: float = side * 0.40
		var x1: float = side * 0.63
		builder.quad(
			Vector3(x0, 0.005, -1.874), Vector3(x1, 0.005, -1.874),
			Vector3(x1, 0.095, -1.874), Vector3(x0, 0.095, -1.874),
			Color.WHITE, 0, Vector3(0.0, 0.0, -1.0))


# --- wheels -------------------------------------------------------------------------------------

func _build_wheel(builder: MeshBuilder, look: Look) -> void:
	# Tire: 12-sided cylinder on +Y (the wheel node's `Mesh` child rotates it onto X).
	_add_cylinder(builder, WHEEL_RADIUS, WHEEL_WIDTH, WHEEL_SIDES, look.tire, 0, false)
	# Hub: a lighter, slightly wider disk; alternate cap wedges read as spokes.
	_add_cylinder(builder, HUB_RADIUS, HUB_WIDTH, HUB_SIDES, look.hub, 1, true)


func _add_cylinder(builder: MeshBuilder, radius: float, width: float, sides: int, color: Color, group: int, stripe: bool) -> void:
	var half := width * 0.5
	var top: Array[Vector3] = []
	var bottom: Array[Vector3] = []
	for i in sides:
		var angle := TAU * float(i) / float(sides)
		var x := cos(angle) * radius
		var z := sin(angle) * radius
		top.append(Vector3(x, half, z))
		bottom.append(Vector3(x, -half, z))
	for i in sides:
		var j := (i + 1) % sides
		builder.quad(bottom[i], bottom[j], top[j], top[i], color, group, Vector3(bottom[i].x, 0.0, bottom[i].z))
	var top_center := Vector3(0.0, half, 0.0)
	var bottom_center := Vector3(0.0, -half, 0.0)
	for i in sides:
		var j := (i + 1) % sides
		var cap_color := color
		if stripe and i % 2 == 1:
			cap_color = color.darkened(0.28)
		builder.tri(top_center, top[i], top[j], cap_color, group, Vector3.UP)
		builder.tri(bottom_center, bottom[j], bottom[i], cap_color, group, Vector3.DOWN)


# --- shared geometry ----------------------------------------------------------------------------

## Extrudes a (z, y) profile across X. The middle is one quad per edge; the two side caps are the
## profile inset by `bevel`, joined by `segments` chamfer bands per side.
func _add_prism(builder: MeshBuilder, profile: PackedVector2Array, half_width: float, bevel: float, segments: int, color: Color, group: int) -> void:
	var n := profile.size()
	if n < 3:
		return
	var center2 := Vector2.ZERO
	for point in profile:
		center2 += point
	center2 /= float(n)
	var center := Vector3(0.0, center2.y, center2.x)
	var chamfer := clampf(bevel, 0.0, half_width * 0.45)
	var inner := half_width - chamfer

	# Straight middle between the chamfers.
	for i in n:
		var a := profile[i]
		var b := profile[(i + 1) % n]
		var va0 := Vector3(-inner, a.y, a.x)
		var vb0 := Vector3(-inner, b.y, b.x)
		var vb1 := Vector3(inner, b.y, b.x)
		var va1 := Vector3(inner, a.y, a.x)
		builder.quad(va0, vb0, vb1, va1, color, group, _hint(va0, vb0, vb1, center))

	# Chamfered sides and flat caps.
	for side_value in [-1.0, 1.0]:
		var side: float = side_value
		var xs: Array[float] = []
		var rings: Array[PackedVector2Array] = []
		for s in segments:
			var f := float(s) / float(segments)
			xs.append(side * (inner + chamfer * f))
			rings.append(_inset_profile(profile, chamfer * f))
		xs.append(side * half_width)
		rings.append(_inset_profile(profile, chamfer))
		for r in range(xs.size() - 1):
			var x0 := xs[r]
			var x1 := xs[r + 1]
			var prof0 := rings[r]
			var prof1 := rings[r + 1]
			for i in n:
				var j := (i + 1) % n
				var a0 := prof0[i]
				var b0 := prof0[j]
				var a1 := prof1[i]
				var b1 := prof1[j]
				var p0 := Vector3(x0, a0.y, a0.x)
				var p1 := Vector3(x0, b0.y, b0.x)
				var p2 := Vector3(x1, b1.y, b1.x)
				var p3 := Vector3(x1, a1.y, a1.x)
				builder.quad(p0, p1, p2, p3, color, group, _hint(p0, p1, p2, center))
		var cap := rings[rings.size() - 1]
		var x: float = side * half_width
		var cap0 := Vector3(x, cap[0].y, cap[0].x)
		for i in range(1, n - 1):
			var cap1 := Vector3(x, cap[i].y, cap[i].x)
			var cap2 := Vector3(x, cap[i + 1].y, cap[i + 1].x)
			builder.tri(cap0, cap1, cap2, color, group, Vector3(side, 0.0, 0.0))


func _add_box(builder: MeshBuilder, center: Vector3, size: Vector3, color: Color, group: int) -> void:
	var h := size * 0.5
	var p := [
		center + Vector3(-h.x, -h.y, -h.z),
		center + Vector3(h.x, -h.y, -h.z),
		center + Vector3(h.x, -h.y, h.z),
		center + Vector3(-h.x, -h.y, h.z),
		center + Vector3(-h.x, h.y, -h.z),
		center + Vector3(h.x, h.y, -h.z),
		center + Vector3(h.x, h.y, h.z),
		center + Vector3(-h.x, h.y, h.z),
	]
	builder.quad(p[0], p[1], p[5], p[4], color, group, Vector3(0.0, 0.0, -1.0))
	builder.quad(p[2], p[3], p[7], p[6], color, group, Vector3(0.0, 0.0, 1.0))
	builder.quad(p[3], p[0], p[4], p[7], color, group, Vector3(-1.0, 0.0, 0.0))
	builder.quad(p[1], p[2], p[6], p[5], color, group, Vector3(1.0, 0.0, 0.0))
	builder.quad(p[3], p[2], p[1], p[0], color, group, Vector3(0.0, -1.0, 0.0))
	builder.quad(p[4], p[5], p[6], p[7], color, group, Vector3(0.0, 1.0, 0.0))


## Quadratic rounding of each polygon corner; `segments` is the number of arc pieces.
func _round_profile(points: PackedVector2Array, radius: float, segments: int) -> PackedVector2Array:
	var n := points.size()
	var out := PackedVector2Array()
	for i in n:
		var point := points[i]
		var prev := points[(i - 1 + n) % n]
		var next := points[(i + 1) % n]
		var to_prev := prev - point
		var to_next := next - point
		var length_prev := to_prev.length()
		var length_next := to_next.length()
		if length_prev < 1e-6 or length_next < 1e-6:
			out.append(point)
			continue
		var r := minf(radius, minf(length_prev, length_next) * 0.45)
		var a := point + to_prev / length_prev * r
		var b := point + to_next / length_next * r
		for s in segments + 1:
			var t := float(s) / float(segments)
			out.append(a.lerp(point, t).lerp(point.lerp(b, t), t))
	return out


## Moves each profile point inward by `amount` along its outward 2D normal.
func _inset_profile(profile: PackedVector2Array, amount: float) -> PackedVector2Array:
	if amount <= 0.0:
		return profile.duplicate()
	var n := profile.size()
	var ccw := _is_ccw(profile)
	var out := PackedVector2Array()
	for i in n:
		var point := profile[i]
		var prev := profile[(i - 1 + n) % n]
		var next := profile[(i + 1) % n]
		var dir_prev := (point - prev).normalized()
		var dir_next := (next - point).normalized()
		var normal_prev := Vector2(dir_prev.y, -dir_prev.x) if ccw else Vector2(-dir_prev.y, dir_prev.x)
		var normal_next := Vector2(dir_next.y, -dir_next.x) if ccw else Vector2(-dir_next.y, dir_next.x)
		out.append(point - (normal_prev + normal_next).normalized() * amount)
	return out


func _is_ccw(profile: PackedVector2Array) -> bool:
	var area := 0.0
	for i in profile.size():
		var a := profile[i]
		var b := profile[(i + 1) % profile.size()]
		area += a.x * b.y - b.x * a.y
	return area > 0.0


## Outward normal of a clockwise profile edge (z, y).
func _outward_2d(a: Vector2, b: Vector2) -> Vector2:
	var edge := b - a
	return Vector2(-edge.y, edge.x).normalized()


## Outward direction of a face of a convex solid: from its center to the face centroid.
func _hint(a: Vector3, b: Vector3, c: Vector3, center: Vector3) -> Vector3:
	var outward := (a + b + c) / 3.0 - center
	return outward if outward.length_squared() > 1e-12 else Vector3.UP


# --- materials ----------------------------------------------------------------------------------

func _body_material() -> StandardMaterial3D:
	var mat := StandardMaterial3D.new()
	mat.vertex_color_use_as_albedo = true
	mat.roughness = 0.48
	mat.metallic = 0.06
	mat.metallic_specular = 0.4
	return mat


func _wheel_material() -> StandardMaterial3D:
	var mat := StandardMaterial3D.new()
	mat.vertex_color_use_as_albedo = true
	mat.roughness = 0.78
	mat.metallic = 0.0
	mat.metallic_specular = 0.25
	return mat


func _glass_material(look: Look) -> StandardMaterial3D:
	var mat := StandardMaterial3D.new()
	mat.albedo_color = look.glass
	mat.roughness = 0.10
	mat.metallic = 0.35
	mat.metallic_specular = 0.65
	return mat


func _headlamp_material(look: Look) -> StandardMaterial3D:
	var mat := StandardMaterial3D.new()
	mat.albedo_color = look.lamp
	mat.roughness = 0.18
	mat.emission_enabled = true
	mat.emission = look.lamp
	mat.emission_energy_multiplier = 0.12
	return mat
