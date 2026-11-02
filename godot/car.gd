extends VehicleBody3D

## Motor e direção simples. Entradas vêm das ações drive_* do InputMap.
## Frente do carro é -Z; rodas dianteiras esterçam, traseiras tracionam.
## Com engine_force positivo o VehicleBody3D anda para +Z, então o sinal é invertido.

@export var max_engine_force := 800.0
@export var max_brake_force := 30.0
@export var max_steer := 0.5


func _physics_process(_delta: float) -> void:
	var throttle := Input.get_axis("drive_back", "drive_forward")
	var steer_input := Input.get_axis("drive_right", "drive_left")

	engine_force = -throttle * max_engine_force
	brake = max_brake_force if is_zero_approx(throttle) else 0.0
	steering = steer_input * max_steer
