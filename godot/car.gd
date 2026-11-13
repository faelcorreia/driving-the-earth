extends VehicleBody3D

const LowpolyCar := preload("res://lowpoly_car.gd")

## Motor e direção simples. Entradas vêm das ações drive_* do InputMap.
## Frente do carro é -Z; rodas dianteiras esterçam, traseiras tracionam.
## Com engine_force positivo o VehicleBody3D anda para +Z, então o sinal é invertido.
##
## Performance target: light 1.0 L hatchback, 0-100 km/h in ~14.5 s, top speed ~100 km/h, on flat ground.
## Traction falls linearly from `max_traction_force_n` (standstill) to zero at `top_speed_kmh * 1.1`;
## aero drag and rolling resistance hold the final speed near `top_speed_kmh`.
## VehicleBody3D also resists speed linearly (~`wheel_damping_n_per_ms`, measured), so the engine
## compensates for it: the curve above is what the car actually does.

@export var top_speed_kmh := 100.0  ## target final speed on flat ground, km/h (~28.7 m/s measured)
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

## Visões da câmera, em ordem de ciclo pela tecla C (drive_camera). Cada uma tem `set_active(on)`.
@onready var _views: Array = [$Camera, $DriverCamera]
var _view_index := 0

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

## Lights. Headlights and tail lamps follow the street lamps' global (`lamp_state`, no per-pole
## delay). The brake light is brighter while braking or reversing against the motion.
const TAIL_EMISSION_ON := 1.2
const TAIL_EMISSION_BRAKE := 4.0
## Speed (m/s) above which an input against the motion counts as braking.
const BRAKE_MIN_SPEED := 0.5
@onready var _headlights: Array[SpotLight3D] = [$HeadlightLeft, $HeadlightRight]
@onready var _tails: Array[MeshInstance3D] = [$TailLeft, $TailRight]
var _tail_mats: Array[StandardMaterial3D] = []
## Headlamp lens material (item 120), glows while the headlights are on.
var _headlamp_mats: Array[StandardMaterial3D] = []
var _braking := false
var _clock: Node = null
## The world's Gps node (main.tscn), null when the car has none.
var _gps: Object = null
## m/s to km/h.
const KMH_PER_MS := 3.6
var _hud_value: Label = null
var _hud_unit: Label = null
var _shown_kmh := -1


func _ready() -> void:
	_gps = get_parent().find_child("Gps", false, false)
	for tail in _tails:
		var mat := StandardMaterial3D.new()
		mat.albedo_color = Color(0.35, 0.02, 0.02)
		mat.emission_enabled = true
		mat.emission = Color(1.0, 0.05, 0.02)
		mat.emission_energy_multiplier = 0.0
		tail.material_override = mat
		_tail_mats.append(mat)
	var points := PackedVector3Array()
	for p in _cabin_profile:
		points.append(Vector3(-CABIN_HALF_WIDTH, p.y, p.x))
		points.append(Vector3(CABIN_HALF_WIDTH, p.y, p.x))
	var shape := ConvexPolygonShape3D.new()
	shape.points = points
	$CabinShape.shape = shape
	# Visible car (item 120): body, cabin, glass, lamps and wheels, built once from the theme.
	var lowpoly := LowpolyCar.new()
	var stats := lowpoly.apply(self, _cabin_profile, CABIN_HALF_WIDTH)
	_headlamp_mats.append(stats["headlamp_material"] as StandardMaterial3D)
	if "--car-stats" in OS.get_cmdline_user_args():
		print("car: %d triangles (body %d, cabin %d, wheels %d, lamps %d); model %s; style '%s', flat shading %s, bevel %.3f m" % [
			stats["triangles"], stats["body"], stats["cabin"], stats["wheels"], stats["lamps"],
			str(stats["model"]), stats["style_id"], str(stats["flat"]), stats["bevel"],
		])
	Input.mouse_mode = Input.MOUSE_MODE_CAPTURED
	_set_view(0)


## Troca a visão ativa. Desliga as outras antes de ligar a nova, para só uma câmera ficar `current`.
func _set_view(index: int) -> void:
	_view_index = index
	for i in _views.size():
		if i != index:
			_views[i].set_active(false)
	_views[index].set_active(true)


func _unhandled_input(event: InputEvent) -> void:
	if event.is_action_pressed("drive_reset"):
		_reset_requested = true
	if event.is_action_pressed("drive_camera"):
		_set_view((_view_index + 1) % _views.size())
	if event.is_action_pressed("ui_cancel"):
		if Input.mouse_mode == Input.MOUSE_MODE_CAPTURED:
			Input.mouse_mode = Input.MOUSE_MODE_VISIBLE
		else:
			Input.mouse_mode = Input.MOUSE_MODE_CAPTURED
		get_viewport().set_input_as_handled()
		return
	var click := event as InputEventMouseButton
	if click and click.pressed and click.button_index == MOUSE_BUTTON_LEFT:
		Input.mouse_mode = Input.MOUSE_MODE_CAPTURED
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
	_braking = (throttle < 0.0 and forward_speed > BRAKE_MIN_SPEED) or (throttle > 0.0 and forward_speed < -BRAKE_MIN_SPEED)

	# GPS (item 112): the car's position in the map's local meters (x east, y north = -z), once per tick.
	if _gps != null:
		var pos := global_transform.origin
		_gps.call("update", pos.x, -pos.z)


## The environment clock, found by name among the world's children once it exists (null before).
func _environment() -> Node:
	if _clock == null:
		_clock = get_parent().find_child("Environment", false, false)
	return _clock


## True while the street lamps are on: asks the environment clock (the state the lamps use, without
## the per-pole delay).
func _lamps_on() -> bool:
	var env := _environment()
	if env == null:
		return false
	return env.lamps_on()


## Speed readout in the HUD (group `speed_hud`, see main.tscn). The number is rewritten only when the
## whole km/h value changes; the unit comes from the clock's i18n lookup once both exist.
func _update_hud() -> void:
	if _hud_value == null:
		var hud := get_tree().get_first_node_in_group("speed_hud")
		var env := _environment()
		if hud == null or env == null:
			return
		_hud_value = hud.get_node("Readout/Value") as Label
		_hud_unit = hud.get_node("Readout/Unit") as Label
		_hud_unit.text = env.hud_text("hud.speed_unit")
	var kmh := int(roundf(linear_velocity.length() * KMH_PER_MS))
	if kmh != _shown_kmh:
		_shown_kmh = kmh
		_hud_value.text = str(kmh)


func _process(_delta: float) -> void:
	var on := _lamps_on()
	for light in _headlights:
		light.visible = on
	for mat in _headlamp_mats:
		mat.emission_energy_multiplier = 1.4 if on else 0.12
	for mat in _tail_mats:
		if _braking:
			mat.emission_energy_multiplier = TAIL_EMISSION_BRAKE
		else:
			mat.emission_energy_multiplier = TAIL_EMISSION_ON if on else 0.0
	_update_hud()
