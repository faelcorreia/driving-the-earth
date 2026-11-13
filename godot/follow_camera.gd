extends Camera3D

## Third-person camera like GTA. Orbits the car (the parent) on a sphere around a pivot above it.
## Yaw and elevation belong to the player: turning the car does not turn the camera. After
## `recenter_delay` seconds without mouse input, while the car is moving, the yaw eases back behind
## the car. The camera position is always `pivot + sphere(yaw, elevation, distance)`, except when a
## collision pulls it closer to the pivot. Turned on and off by the car (set_active); while off it
## runs no input and no physics.

@export var mouse_sensitivity := 0.003  ## rad per mouse pixel
@export var distance := 6.0  ## m between the pivot and the camera
@export var height := 2.0  ## m above the car's origin (the pivot)
@export var min_elevation_deg := -20.0  ## negative = camera below the pivot, looking up
@export var max_elevation_deg := 80.0  ## positive = camera above the pivot, looking down
@export var initial_elevation_deg := 15.0
@export var recenter_delay := 2.0  ## s without mouse before the yaw recenters
@export var recenter_speed := 2.0  ## recenter smoothing (exponential lerp)
@export var recenter_min_speed := 1.0  ## m/s; below this the car counts as stopped
@export var collision_margin := 0.3  ## m of clearance between the camera and an obstacle

var _car: VehicleBody3D
var _yaw := 0.0
var _elevation := 0.0
var _idle := 0.0
var _initialized := false


func _ready() -> void:
	# Outside the car's hierarchy, so it does not inherit the car's rotation.
	top_level = true
	_car = get_parent() as VehicleBody3D
	_elevation = deg_to_rad(initial_elevation_deg)


func _unhandled_input(event: InputEvent) -> void:
	var motion := event as InputEventMouseMotion
	if motion and Input.mouse_mode == Input.MOUSE_MODE_CAPTURED:
		orbit(motion.relative)


## Rotates the orbit by a mouse motion, in pixels. Kept apart from `_unhandled_input` so the
## headless camera check can use the same path (headless has no mouse capture).
func orbit(relative: Vector2) -> void:
	_yaw -= relative.x * mouse_sensitivity
	# Mouse down looks down (the camera rises above the pivot); mouse up looks up.
	_elevation = clampf(
		_elevation + relative.y * mouse_sensitivity,
		deg_to_rad(min_elevation_deg),
		deg_to_rad(max_elevation_deg)
	)
	_idle = 0.0


## Turns this view on or off. While off, the camera receives no input and runs no `_physics_process`.
func set_active(on: bool) -> void:
	current = on
	process_mode = Node.PROCESS_MODE_INHERIT if on else Node.PROCESS_MODE_DISABLED


func _physics_process(delta: float) -> void:
	if _car == null:
		return

	var car_yaw := _car.global_rotation.y
	var pivot := _car.global_position + Vector3.UP * height
	if not _initialized:
		_initialized = true
		_yaw = car_yaw

	_idle += delta
	if _idle >= recenter_delay and _car.linear_velocity.length() >= recenter_min_speed:
		_yaw = lerp_angle(_yaw, car_yaw, 1.0 - exp(-recenter_speed * delta))

	# Sphere around the pivot: yaw 0 sits behind the car (+Z), elevation 0 is level with the pivot.
	var flat := cos(_elevation)
	var offset := Vector3(sin(_yaw) * flat, sin(_elevation), cos(_yaw) * flat) * distance
	global_position = _clip_to_obstacle(pivot, pivot + offset)
	# No roll: the camera always looks at the pivot with world up.
	look_at(pivot, Vector3.UP)


## Pulls the camera back to the first obstacle between the pivot and its desired position.
## The car itself is ignored.
func _clip_to_obstacle(from: Vector3, to: Vector3) -> Vector3:
	var query := PhysicsRayQueryParameters3D.create(from, to)
	if _car:
		query.exclude = [_car.get_rid()]
	var hit := get_world_3d().direct_space_state.intersect_ray(query)
	if hit.is_empty():
		return to
	return hit.position + hit.normal * collision_margin
