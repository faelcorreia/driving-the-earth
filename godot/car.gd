extends VehicleBody3D

## Motor e direção simples. Entradas vêm das ações drive_* do InputMap.
## Frente do carro é -Z; rodas dianteiras esterçam, traseiras tracionam.
## Com engine_force positivo o VehicleBody3D anda para +Z, então o sinal é invertido.
##
## Performance target: Fiat Uno Mille, 0-100 km/h in ~14.5 s, top speed ~120 km/h, on flat ground.
## Traction falls linearly from `max_traction_force_n` (standstill) to zero at `top_speed_kmh * 1.1`;
## aero drag and rolling resistance hold the final speed near `top_speed_kmh`.
## VehicleBody3D also resists speed linearly (~`wheel_damping_n_per_ms`, measured), so the engine
## compensates for it: the curve above is what the car actually does.

@export var top_speed_kmh := 120.0  ## target final speed on flat ground, km/h
@export var max_traction_force_n := 3400.0  ## traction at standstill, N (total across traction wheels)
@export var reverse_force_factor := 0.5  ## reverse uses this fraction of the traction force
@export var aero_drag_k := 0.15  ## aerodynamic drag, N per (m/s)^2
@export var rolling_resistance_n := 125.0  ## rolling resistance, N, constant while moving
@export var wheel_damping_n_per_ms := 87.0  ## VehicleBody3D internal resistance, N per m/s (measured)
@export var traction_wheel_count := 2.0  ## Godot applies engine_force to each traction wheel
@export var max_brake_force := 30.0
@export var max_steer := 0.5
## Esterço reduzido em alta velocidade: com `full_steer_speed` m/s ou mais, o esterço cai
## para `min_steer_factor` do máximo. Evita curvas fechadas a alta velocidade, que viram o carro.
@export var min_steer_factor := 0.4
@export var full_steer_speed := 33.0  ## m/s (~120 km/h)
## Altura extra, em metros, ao endireitar o carro com drive_reset.
@export var reset_lift_m := 1.0

## Cabine (teto, para-brisa e vidro traseiro) como prisma convexo extrudado em X.
## Perfil em (z, y) do carro, com frente em -Z. Convexo, para a colisão ser um ConvexPolygonShape3D.
var _cabin_profile := PackedVector2Array([
	Vector2(-0.85, 0.31),  # base do para-brisa, sobre o capô
	Vector2(-0.35, 0.94),  # topo do para-brisa
	Vector2(1.55, 0.94),  # topo do teto, início do vidro traseiro
	Vector2(1.85, 0.55),  # fim do vidro traseiro, traseira curta
	Vector2(1.85, 0.31),  # traseira, base da cabine
])
const CABIN_HALF_WIDTH := 0.725

var _reset_requested := false


func _ready() -> void:
	var points := PackedVector3Array()
	for p in _cabin_profile:
		points.append(Vector3(-CABIN_HALF_WIDTH, p.y, p.x))
		points.append(Vector3(CABIN_HALF_WIDTH, p.y, p.x))
	var shape := ConvexPolygonShape3D.new()
	shape.points = points
	$CabinShape.shape = shape
	$CabinMesh.mesh = _prism_mesh(_cabin_profile, CABIN_HALF_WIDTH)


## Malha de um prisma convexo: perfil (x = z do carro, y = altura) extrudado de -w a +w em X.
## Cada triângulo sai com a normal para fora e ordem horária vista de fora (frente no Godot).
func _prism_mesh(profile: PackedVector2Array, half_width: float) -> ArrayMesh:
	var centroid := Vector3.ZERO
	for p in profile:
		centroid += Vector3(0.0, p.y, p.x)
	centroid /= profile.size()

	var st := SurfaceTool.new()
	st.begin(Mesh.PRIMITIVE_TRIANGLES)
	var n := profile.size()
	for i in n:
		var a := profile[i]
		var b := profile[(i + 1) % n]
		var la := Vector3(-half_width, a.y, a.x)
		var lb := Vector3(-half_width, b.y, b.x)
		var ra := Vector3(half_width, a.y, a.x)
		var rb := Vector3(half_width, b.y, b.x)
		_add_face(st, centroid, la, lb, rb)
		_add_face(st, centroid, la, rb, ra)
	for i in range(1, n - 1):
		var p0 := profile[0]
		var p1 := profile[i]
		var p2 := profile[i + 1]
		_add_face(st, centroid, Vector3(-half_width, p0.y, p0.x), Vector3(-half_width, p1.y, p1.x), Vector3(-half_width, p2.y, p2.x))
		_add_face(st, centroid, Vector3(half_width, p0.y, p0.x), Vector3(half_width, p2.y, p2.x), Vector3(half_width, p1.y, p1.x))
	return st.commit()


func _add_face(st: SurfaceTool, centroid: Vector3, a: Vector3, b: Vector3, c: Vector3) -> void:
	var normal := (b - a).cross(c - a).normalized()
	if normal.dot((a + b + c) / 3.0 - centroid) < 0.0:
		normal = -normal
	# Godot considera de frente o triângulo horário visto de fora, ou seja, cross(b-a, c-a) aponta para dentro.
	if (b - a).cross(c - a).dot(normal) > 0.0:
		var tmp := b
		b = c
		c = tmp
	for v in [a, b, c]:
		st.set_normal(normal)
		st.add_vertex(v)


func _unhandled_input(event: InputEvent) -> void:
	if event.is_action_pressed("drive_reset"):
		_reset_requested = true
	if event.is_action_pressed("debug_position"):
		# Map local meters: x east, y north (= -z), home at the origin.
		var pos := global_transform.origin
		var forward := -global_transform.basis.z
		var yaw_deg := rad_to_deg(atan2(-forward.x, -forward.z))
		print("Position x=%.2f y=%.2f height=%.2f m, yaw=%.1f deg" % [pos.x, -pos.z, pos.y, yaw_deg])


## Endireita o carro na posição atual, mantendo o yaw, e zera as velocidades.
## Roda no passo de física, onde o estado do corpo pode ser alterado com segurança.
func _integrate_forces(state: PhysicsDirectBodyState3D) -> void:
	if _reset_requested:
		_reset_requested = false

		var forward := -state.transform.basis.z
		var yaw := atan2(-forward.x, -forward.z)
		var origin := state.transform.origin + Vector3.UP * reset_lift_m
		state.transform = Transform3D(Basis.from_euler(Vector3(0.0, yaw, 0.0)), origin)
		state.linear_velocity = Vector3.ZERO
		state.angular_velocity = Vector3.ZERO
		return

	# Aerodynamic drag and rolling resistance, opposing the motion.
	var velocity := state.linear_velocity
	var speed := velocity.length()
	if speed > 0.05:
		var resistance := aero_drag_k * speed * speed + rolling_resistance_n
		state.apply_central_force(-velocity / speed * resistance)


## Traction (N, positive forward) as a function of forward speed.
## Falls linearly from `max_traction_force_n` to zero at `top_speed_kmh * 1.1`;
## drag holds the final speed slightly below that point.
func _traction_force(speed_ms: float) -> float:
	var cutoff_ms := top_speed_kmh * 1.1 / 3.6
	return max_traction_force_n * clampf(1.0 - speed_ms / cutoff_ms, 0.0, 1.0)


func _physics_process(_delta: float) -> void:
	var throttle := Input.get_axis("drive_back", "drive_forward")
	var steer_input := Input.get_axis("drive_right", "drive_left")

	var forward_speed := linear_velocity.dot(-basis.z)
	var drive := 0.0  ## N, positive forward
	var damping_comp := 0.0  ## N, cancels the VehicleBody3D internal resistance
	if throttle > 0.0:
		drive = _traction_force(maxf(forward_speed, 0.0)) * throttle
		damping_comp = wheel_damping_n_per_ms * maxf(forward_speed, 0.0) * throttle
	elif throttle < 0.0:
		drive = _traction_force(maxf(-forward_speed, 0.0)) * reverse_force_factor * throttle
		damping_comp = wheel_damping_n_per_ms * maxf(-forward_speed, 0.0) * throttle

	engine_force = -(drive + damping_comp) / traction_wheel_count
	brake = max_brake_force if is_zero_approx(throttle) else 0.0
	var speed_ratio := clampf(linear_velocity.length() / full_steer_speed, 0.0, 1.0)
	steering = steer_input * max_steer * lerpf(1.0, min_steer_factor, speed_ratio)
