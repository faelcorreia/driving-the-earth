extends SceneTree

## Camera check (item 128): exercises the third-person orbit camera headless.
## It builds its own flat ground, slope and wall, then checks:
## - the camera is `pivot + sphere(yaw, elevation, distance)` at several yaw/elevation pairs;
## - yaw 0 puts the camera behind the car (+Z) and the camera always looks at the pivot without roll;
## - mouse down raises the elevation (camera above the pivot), mouse up lowers it;
## - the yaw recenters only while the car moves (>= recenter_min_speed);
## - on a slope and against a wall the camera stays above the ground and in front of the obstacle.
## Run:
##   env -u DISPLAY -u WAYLAND_DISPLAY godot --headless --path godot -s tools/camera_check.gd
## Exit code 0 when every check passes, 1 otherwise.

const CameraScript := preload("res://follow_camera.gd")
const DISTANCE := 6.0
const HEIGHT := 2.0
const TOLERANCE_M := 0.01
const TOLERANCE_DOT := 0.001
const TOLERANCE_ROLL := 0.001

var _checks := 0
var _failures := 0


func _initialize() -> void:
	run()


func run() -> void:
	# Minimal physics world: the camera only needs a car (its parent) and a collision body.
	var world := Node3D.new()
	world.name = "CameraCheck"
	root.add_child(world)

	var ground := _make_ground(world)
	var car := VehicleBody3D.new()
	car.name = "Car"
	car.freeze = true
	car.freeze_mode = RigidBody3D.FREEZE_MODE_STATIC
	world.add_child(car)
	car.position = Vector3(0.0, 1.5, 0.0)

	var cam := Camera3D.new()
	cam.name = "Camera"
	cam.set_script(CameraScript)
	cam.distance = DISTANCE
	cam.height = HEIGHT
	cam.initial_elevation_deg = 15.0
	cam.recenter_delay = 0.2
	cam.recenter_speed = 4.0
	car.add_child(cam)
	cam.set_active(true)
	await physics_frame
	await physics_frame

	# Distance to the pivot, camera direction and roll, at several yaw/elevation pairs.
	for yaw_deg in [-150.0, -60.0, 0.0, 45.0, 150.0]:
		for elevation_deg in [-19.0, 0.0, 15.0, 45.0, 79.0]:
			await _check_orbit(cam, car, yaw_deg, elevation_deg)

	# Convention: yaw 0 is behind the car (+Z, front is -Z).
	cam._yaw = 0.0
	cam._elevation = deg_to_rad(15.0)
	await physics_frame
	var pivot := _pivot(car)
	_expect(
		cam.global_position.z > pivot.z,
		"yaw 0 puts the camera behind the car (camera z %.2f, pivot z %.2f)" % [
			cam.global_position.z, pivot.z]
	)

	# Vertical mouse: down (+y) raises the camera above the pivot, up (-y) lowers it.
	cam._yaw = 0.0
	cam._elevation = deg_to_rad(15.0)
	await physics_frame
	var elevation_before: float = cam._elevation
	var height_before: float = cam.global_position.y - _pivot(car).y
	cam.orbit(Vector2(0.0, 10.0))
	await physics_frame
	_expect(
		cam._elevation > elevation_before and cam.global_position.y > _pivot(car).y,
		"mouse down raises the camera (elevation %.1f -> %.1f deg, offset %.2f -> %.2f m)" % [
			rad_to_deg(elevation_before), rad_to_deg(cam._elevation),
			height_before, cam.global_position.y - _pivot(car).y]
	)
	elevation_before = cam._elevation
	height_before = cam.global_position.y - _pivot(car).y
	cam.orbit(Vector2(0.0, -10.0))
	await physics_frame
	_expect(
		cam._elevation < elevation_before and cam.global_position.y < _pivot(car).y + height_before,
		"mouse up lowers the camera (elevation %.1f -> %.1f deg, offset %.2f -> %.2f m)" % [
			rad_to_deg(elevation_before), rad_to_deg(cam._elevation),
			height_before, cam.global_position.y - _pivot(car).y]
	)

	# Recenter: not while stopped, yes while moving.
	cam._yaw = deg_to_rad(90.0)
	cam._elevation = deg_to_rad(15.0)
	cam._idle = cam.recenter_delay + 1.0
	var stopped_yaw: float = cam._yaw
	for i in 30:
		await physics_frame
	_expect(
		absf(angle_difference(stopped_yaw, cam._yaw)) < 0.0001,
		"yaw stays put while the car is stopped (%.1f -> %.1f deg)" % [
			rad_to_deg(stopped_yaw), rad_to_deg(cam._yaw)]
	)

	cam._yaw = deg_to_rad(90.0)
	cam._idle = cam.recenter_delay + 1.0
	var moving_start: float = cam._yaw
	car.freeze = false
	car.gravity_scale = 0.0
	for i in 60:
		car.linear_velocity = Vector3(0.0, 0.0, 2.0)
		await physics_frame
	var moving_end: float = cam._yaw
	_expect(
		absf(angle_difference(moving_end, car.global_rotation.y))
			< absf(angle_difference(moving_start, car.global_rotation.y)) - deg_to_rad(10.0),
		"yaw recenters while the car moves (%.1f -> %.1f deg, car %.1f deg)" % [
			rad_to_deg(moving_start), rad_to_deg(moving_end), rad_to_deg(car.global_rotation.y)]
	)

	# A wall behind the camera: the ray from the pivot must stop the camera in front of it.
	car.freeze = true
	car.linear_velocity = Vector3.ZERO
	car.global_position = Vector3(0.0, 1.5, 0.0)
	car.global_rotation = Vector3.ZERO
	ground.rotation = Vector3.ZERO
	var wall := _make_wall(world, Vector3(0.0, 5.0, 4.0))
	await physics_frame
	cam._yaw = 0.0
	cam._elevation = 0.0
	cam._idle = cam.recenter_delay + 1.0
	await physics_frame
	var wall_pivot := _pivot(car)
	var wall_distance := cam.global_position.distance_to(wall_pivot)
	_expect(
		cam.global_position.z < 3.5 and wall_distance < DISTANCE - 0.5,
		"wall stops the camera in front of it (z %.2f m, distance %.2f m)" % [
			cam.global_position.z, wall_distance]
	)
	wall.queue_free()
	await physics_frame

	# A rising slope behind the car with the camera at its lowest elevation: the ground must stop it.
	car.global_position = Vector3(0.0, 1.5, 0.0)
	ground.rotation = Vector3(deg_to_rad(-25.0), 0.0, 0.0)
	await physics_frame
	cam._yaw = 0.0
	cam._elevation = deg_to_rad(-20.0)
	cam._idle = cam.recenter_delay + 1.0
	await physics_frame
	var slope_pivot := _pivot(car)
	var ground_y := _ground_y_at(cam, car, cam.global_position)
	_expect(
		cam.global_position.y >= ground_y + 0.05,
		"camera stays above the slope (camera y %.2f m, ground y %.2f m)" % [
			cam.global_position.y, ground_y]
	)
	_expect(
		cam.global_position.distance_to(slope_pivot) <= DISTANCE + TOLERANCE_M,
		"slope only pulls the camera closer (distance %.2f m)" % [
			cam.global_position.distance_to(slope_pivot)]
	)

	print("camera check: %d checks, %d failures" % [_checks, _failures])
	world.queue_free()
	await process_frame
	quit(0 if _failures == 0 else 1)


func _check_orbit(cam: Camera3D, car: VehicleBody3D, yaw_deg: float, elevation_deg: float) -> void:
	cam._yaw = deg_to_rad(yaw_deg)
	cam._elevation = deg_to_rad(elevation_deg)
	await physics_frame
	var pivot := _pivot(car)
	var dist := cam.global_position.distance_to(pivot)
	var to_pivot := (pivot - cam.global_position).normalized()
	var look := -cam.global_transform.basis.z
	_expect(
		absf(dist - DISTANCE) <= TOLERANCE_M,
		"distance at yaw %.0f deg, elevation %.0f deg: %.3f m (want %.1f m)" % [
			yaw_deg, elevation_deg, dist, DISTANCE]
	)
	_expect(
		look.dot(to_pivot) >= 1.0 - TOLERANCE_DOT,
		"look_at pivot at yaw %.0f deg, elevation %.0f deg (dot %.4f)" % [
			yaw_deg, elevation_deg, look.dot(to_pivot)]
	)
	_expect(
		absf(cam.global_transform.basis.x.y) <= TOLERANCE_ROLL,
		"no roll at yaw %.0f deg, elevation %.0f deg (right.y %.4f)" % [
			yaw_deg, elevation_deg, cam.global_transform.basis.x.y]
	)


func _pivot(car: VehicleBody3D) -> Vector3:
	return car.global_position + Vector3.UP * HEIGHT


## Ground height under `at`, by a ray straight down. The car is ignored.
func _ground_y_at(cam: Camera3D, car: VehicleBody3D, at: Vector3) -> float:
	var query := PhysicsRayQueryParameters3D.create(
		at + Vector3.UP * 100.0, at + Vector3.DOWN * 100.0)
	query.exclude = [car.get_rid()]
	var hit := cam.get_world_3d().direct_space_state.intersect_ray(query)
	return hit.position.y if not hit.is_empty() else -INF


func _make_ground(world: Node3D) -> StaticBody3D:
	var ground := StaticBody3D.new()
	ground.name = "Ground"
	var shape := CollisionShape3D.new()
	var box := BoxShape3D.new()
	box.size = Vector3(200.0, 2.0, 200.0)
	shape.shape = box
	ground.add_child(shape)
	world.add_child(ground)
	ground.position = Vector3(0.0, -1.0, 0.0)
	return ground


func _make_wall(world: Node3D, at: Vector3) -> StaticBody3D:
	var wall := StaticBody3D.new()
	wall.name = "Wall"
	var shape := CollisionShape3D.new()
	var box := BoxShape3D.new()
	box.size = Vector3(10.0, 10.0, 1.0)
	shape.shape = box
	wall.add_child(shape)
	world.add_child(wall)
	wall.position = at
	return wall


func _expect(ok: bool, message: String) -> void:
	_checks += 1
	if ok:
		print("camera check: ok: " + message)
	else:
		_failures += 1
		printerr("camera check: FAIL: " + message)
