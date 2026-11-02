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

var _reset_requested := false


func _unhandled_input(event: InputEvent) -> void:
	if event.is_action_pressed("drive_reset"):
		_reset_requested = true


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
