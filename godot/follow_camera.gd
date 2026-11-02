extends Camera3D

## Câmera de terceira pessoa estilo GTA. Orbita o carro (pai do nó) com free-look pelo mouse.
## Yaw e pitch são do jogador: o carro girar não gira a câmera. Após `recenter_delay` segundos
## sem mexer o mouse, o yaw volta suavemente para trás do carro.
## Esc alterna captura do mouse; clique esquerdo recaptura.

@export var mouse_sensitivity := 0.003  ## rad por pixel de mouse
@export var distance := 6.0  ## m entre o ponto de mira e a câmera
@export var height := 2.0  ## m acima da origem do carro (ponto de mira)
@export var min_pitch_deg := -80.0  ## negativo = olhando para baixo
@export var max_pitch_deg := 20.0  ## positivo = olhando para cima
@export var initial_pitch_deg := -10.0
@export var follow_speed := 8.0  ## suavização da posição de seguimento (lerp exponencial)
@export var recenter_delay := 2.0  ## s sem mouse antes de recentralizar o yaw
@export var recenter_speed := 2.0  ## suavização do recentralizar (lerp exponencial)
@export var collision_margin := 0.3  ## m de folga entre a câmera e o terreno

var _car: VehicleBody3D
var _yaw := 0.0
var _pitch := 0.0
var _pivot := Vector3.ZERO
var _idle := 0.0
var _initialized := false


func _ready() -> void:
	# Fora da hierarquia do carro, para não herdar sua rotação.
	top_level = true
	_car = get_parent() as VehicleBody3D
	_pitch = deg_to_rad(initial_pitch_deg)
	Input.mouse_mode = Input.MOUSE_MODE_CAPTURED


func _unhandled_input(event: InputEvent) -> void:
	var motion := event as InputEventMouseMotion
	if motion and Input.mouse_mode == Input.MOUSE_MODE_CAPTURED:
		_yaw -= motion.relative.x * mouse_sensitivity
		_pitch = clampf(
			_pitch - motion.relative.y * mouse_sensitivity,
			deg_to_rad(min_pitch_deg),
			deg_to_rad(max_pitch_deg)
		)
		_idle = 0.0
		return

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


func _physics_process(delta: float) -> void:
	if _car == null:
		return

	var car_yaw := _car.global_rotation.y
	var target_pivot := _car.global_position + Vector3.UP * height
	if not _initialized:
		_initialized = true
		_pivot = target_pivot
		_yaw = car_yaw

	_pivot = _pivot.lerp(target_pivot, 1.0 - exp(-follow_speed * delta))

	_idle += delta
	if _idle >= recenter_delay:
		_yaw = lerp_angle(_yaw, car_yaw, 1.0 - exp(-recenter_speed * delta))

	# Direção de olhar da câmera; a posição fica do lado oposto, a `distance` do ponto de mira.
	var forward := Vector3(
		-sin(_yaw) * cos(_pitch),
		sin(_pitch),
		-cos(_yaw) * cos(_pitch)
	)
	var desired := _pivot - forward * distance
	global_position = _clip_to_terrain(_pivot, desired)
	look_at(_pivot, Vector3.UP)


## Recua a câmera até o primeiro obstáculo entre o ponto de mira e a posição desejada.
## O próprio carro é ignorado.
func _clip_to_terrain(from: Vector3, to: Vector3) -> Vector3:
	var query := PhysicsRayQueryParameters3D.create(from, to)
	if _car:
		query.exclude = [_car.get_rid()]
	var hit := get_world_3d().direct_space_state.intersect_ray(query)
	if hit.is_empty():
		return to
	return hit.position + hit.normal * collision_margin
