extends VehicleBody3D

## Motor e direção simples. Entradas vêm das ações drive_* do InputMap.
## Frente do carro é -Z; rodas dianteiras esterçam, traseiras tracionam.
## Com engine_force positivo o VehicleBody3D anda para +Z, então o sinal é invertido.

@export var max_engine_force := 800.0
@export var max_brake_force := 30.0
@export var max_steer := 0.5
## Esterço reduzido em alta velocidade: com `full_steer_speed` m/s ou mais, o esterço cai
## para `min_steer_factor` do máximo. Evita curvas fechadas a alta velocidade, que viram o carro.
@export var min_steer_factor := 0.4
@export var full_steer_speed := 25.0  ## m/s
## Altura extra, em metros, ao endireitar o carro com drive_reset.
@export var reset_lift_m := 1.0

var _reset_requested := false


func _unhandled_input(event: InputEvent) -> void:
	if event.is_action_pressed("drive_reset"):
		_reset_requested = true


## Endireita o carro na posição atual, mantendo o yaw, e zera as velocidades.
## Roda no passo de física, onde o estado do corpo pode ser alterado com segurança.
func _integrate_forces(state: PhysicsDirectBodyState3D) -> void:
	if not _reset_requested:
		return
	_reset_requested = false

	var forward := -state.transform.basis.z
	var yaw := atan2(-forward.x, -forward.z)
	var origin := state.transform.origin + Vector3.UP * reset_lift_m
	state.transform = Transform3D(Basis.from_euler(Vector3(0.0, yaw, 0.0)), origin)
	state.linear_velocity = Vector3.ZERO
	state.angular_velocity = Vector3.ZERO


func _physics_process(_delta: float) -> void:
	var throttle := Input.get_axis("drive_back", "drive_forward")
	var steer_input := Input.get_axis("drive_right", "drive_left")

	engine_force = -throttle * max_engine_force
	brake = max_brake_force if is_zero_approx(throttle) else 0.0
	var speed_ratio := clampf(linear_velocity.length() / full_steer_speed, 0.0, 1.0)
	steering = steer_input * max_steer * lerpf(1.0, min_steer_factor, speed_ratio)
