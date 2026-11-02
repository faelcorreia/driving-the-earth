extends Camera3D

## Visão em primeira pessoa, na posição dos olhos do motorista (lado esquerdo do Uno).
## Filha do carro: segue a carroceria sem código por frame. Olhar livre com o mouse, com limites.
## Não roda `_process` nem `_physics_process`: yaw e pitch mudam só nos eventos de mouse.
## Ligada e desligada pelo carro (set_active).

@export var mouse_sensitivity := 0.003  ## rad por pixel de mouse
@export var max_yaw_deg := 120.0  ## olhar para cada lado, a partir da frente do carro
@export var min_pitch_deg := -60.0  ## negativo = olhando para baixo
@export var max_pitch_deg := 30.0  ## positivo = olhando para cima

var _yaw := 0.0
var _pitch := 0.0


func _unhandled_input(event: InputEvent) -> void:
	var motion := event as InputEventMouseMotion
	if motion == null or Input.mouse_mode != Input.MOUSE_MODE_CAPTURED:
		return
	_yaw = clampf(
		_yaw - motion.relative.x * mouse_sensitivity,
		-deg_to_rad(max_yaw_deg),
		deg_to_rad(max_yaw_deg)
	)
	_pitch = clampf(
		_pitch - motion.relative.y * mouse_sensitivity,
		deg_to_rad(min_pitch_deg),
		deg_to_rad(max_pitch_deg)
	)
	rotation = Vector3(_pitch, _yaw, 0.0)


## Liga ou desliga a visão. Ao ligar, olha para a frente do carro.
func set_active(on: bool) -> void:
	current = on
	process_mode = Node.PROCESS_MODE_INHERIT if on else Node.PROCESS_MODE_DISABLED
	if on:
		_yaw = 0.0
		_pitch = 0.0
		rotation = Vector3.ZERO
