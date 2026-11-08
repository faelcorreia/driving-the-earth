extends SceneTree

## Drive bot (item 4): drives a fixed route from the car's spawn (the saved home) and prints the
## distance driven and the standard deviation of the chassis y over a fixed span of simulated time.
## Run:
##   godot --headless --path godot -s tools/drive_bot.gd -- --map casa-10km
## Optional: --seconds 20 (measured span, simulated s), --settle 3 (simulated s with no input before
## the span, so the car settles on its suspension), --length 900 (route length, m), --cruise 33.3
## (top speed the bot asks for, m/s; the route's curves cap it further).
##
## Route: DriveRoute (crates/gdext/src/world/route.rs), from the car's position and heading at spawn.
## Control: pure pursuit on the route, through the drive_* input actions, as a player would.
## Time is counted in physics ticks, so the span does not depend on the frame rate.
##
## Ram mode (item 89): `--ram` (a flag, no value) aims at the nearest building ahead, drives into it,
## reverses after the stop and prints the outcome: "through" (the car went into the building), "stuck"
## (it stayed on the building after reversing for RAM_REVERSE_MAX_S) or "clear" (it backed off
## RAM_CLEAR_M m from the impact within that time). Other options: --cruise, --settle.
## Example: godot --headless --path godot -s tools/drive_bot.gd -- --map casa-10km --ram

const LOOKAHEAD_MIN_M := 6.0
const LOOKAHEAD_MAX_M := 25.0
const LOOKAHEAD_PER_MS := 0.5
const LOOKAHEAD_BASE_M := 4.0
## Lateral acceleration the bot allows itself in curves, m/s².
const CURVE_LAT_ACCEL := 4.0
const SEARCH_AHEAD := 80
## A stop: throttle held while speed stays under STOP_SPEED_MS for more than STOP_HOLD_S (simulated s).
const STOP_SPEED_MS := 1.0
const STOP_HOLD_S := 1.0
## What a wheel touches, per tick (bounce probe): in the air, the terrain, a road ribbon or anything else.
const SURFACE_AIR := 0
const SURFACE_TERRAIN := 1
const SURFACE_ROAD := 2
const SURFACE_OTHER := 3
## Ram mode: the ray that finds the building ahead (from RAM_RAY_HEIGHT_M above the car's origin),
## how many hits on other bodies it skips, how long it waits for a building to load, the time limit of
## the approach and of the reverse (simulated s), and the distance from the impact that counts as clear.
const RAM_RAY_M := 200.0
const RAM_RAY_HEIGHT_M := 0.5
const RAM_RAY_SKIPS := 8
const RAM_BUILDING_WAIT_S := 10.0
const RAM_APPROACH_MAX_S := 30.0
const RAM_REVERSE_MAX_S := 5.0
const RAM_CLEAR_M := 3.0


func _initialize() -> void:
	run()


func _parse_args() -> Dictionary:
	var opts := {
		"map": "casa-10km",
		"seconds": "20",
		"settle": "3",
		"length": "900",
		"cruise": "33.3",
		"offset": "0",
		"sweep": "0",
	}
	var args := OS.get_cmdline_user_args()
	var i := 0
	while i < args.size():
		var key := args[i].trim_prefix("--")
		if opts.has(key) and i + 1 < args.size():
			opts[key] = args[i + 1]
			i += 2
		else:
			i += 1
	return opts


func run() -> void:
	var opts := _parse_args()
	var main: Node = (load("res://main.tscn") as PackedScene).instantiate()
	var world := main.get_node("World")
	# map_id goes on the node before it enters the tree, so the world is built on the right map.
	world.set("map_id", opts["map"])
	root.add_child(main)

	var cam: Camera3D = null
	for i in 120:
		await process_frame
		cam = world.find_child("Camera", true, false) as Camera3D
		if cam != null:
			break
	if cam == null:
		push_error("drive bot: no car camera under World (spawn failed?)")
		await _finish(main, 1)
		return
	var car := cam.get_parent() as VehicleBody3D
	if car == null:
		push_error("drive bot: the camera is not under the car")
		await _finish(main, 1)
		return

	if _has_flag("ram"):
		var code := await _run_ram(car, opts)
		await _finish(main, code)
		return

	var cache_root := ProjectSettings.globalize_path(str(world.get("cache_root")))
	var forward := -car.global_transform.basis.z
	var start := Vector2(car.global_position.x, -car.global_position.z)
	var heading := Vector2(forward.x, -forward.z).normalized()
	var length_m := float(opts["length"])
	var route_tool: Object = ClassDB.instantiate(&"DriveRoute")
	if route_tool == null:
		push_error("drive bot: class DriveRoute is not registered (extension not loaded?)")
		await _finish(main, 1)
		return
	var waypoints: PackedVector2Array = route_tool.call("waypoints", cache_root, opts["map"], start, heading, length_m)
	if waypoints.size() < 2:
		push_error("drive bot: no route from the start point on map '%s'" % opts["map"])
		await _finish(main, 1)
		return
	waypoints = _offset_route(waypoints, float(opts["offset"]), float(opts["sweep"]) > 0.0)
	var route_m := 0.0
	var cum := PackedFloat32Array([0.0])
	for i in range(1, waypoints.size()):
		route_m += waypoints[i].distance_to(waypoints[i - 1])
		cum.append(route_m)
	print("drive bot: map %s, start x=%.2f y=%.2f, route %d points, %.1f m" % [opts["map"], start.x, start.y, waypoints.size(), route_m])

	var tps := float(Engine.physics_ticks_per_second)
	var settle_ticks := int(round(float(opts["settle"]) * tps))
	for i in settle_ticks:
		await physics_frame

	var span_ticks := int(round(float(opts["seconds"]) * tps))
	var wheelbase := absf(car.get_node("FrontLeft").position.z - car.get_node("RearLeft").position.z)
	var state := {"index": 0, "cum": cum, "route": waypoints, "wheelbase": wheelbase, "cruise": float(opts["cruise"])}
	var start_pos := car.global_position
	var prev_pos := start_pos
	var distance_m := 0.0
	var ys := PackedFloat64Array()
	# Per wheel, per tick: the wheel's height in the chassis frame (suspension) and the height of its
	# contact point (ground: terrain or road). NAN when the wheel is in the air.
	var wheels: Array[VehicleWheel3D] = []
	for child in car.get_children():
		var wheel := child as VehicleWheel3D
		if wheel != null:
			wheels.append(wheel)
	var wheel_local_y: Array[PackedFloat64Array] = []
	var wheel_ground_y: Array[PackedFloat64Array] = []
	var wheel_surface: Array[PackedByteArray] = []
	for wheel in wheels:
		wheel_local_y.append(PackedFloat64Array())
		wheel_ground_y.append(PackedFloat64Array())
		wheel_surface.append(PackedByteArray())
	# Contacts of the body are reported only for the bot, so the stops can name what the car hit.
	car.contact_monitor = true
	car.max_contacts_reported = 16
	var stop_ticks := int(round(STOP_HOLD_S * tps))
	var stuck_ticks := 0
	var stops := 0
	for i in span_ticks:
		_steer_and_throttle(car, state)
		await physics_frame
		var pos := car.global_position
		distance_m += Vector2(pos.x - prev_pos.x, pos.z - prev_pos.z).length()
		prev_pos = pos
		ys.append(pos.y)
		for k in wheels.size():
			var wheel := wheels[k]
			wheel_local_y[k].append(car.to_local(wheel.global_position).y)
			if wheel.is_in_contact():
				wheel_ground_y[k].append(wheel.get_contact_point().y)
				var body := wheel.get_contact_body()
				var path := str(body.get_path()) if body else ""
				wheel_surface[k].append(SURFACE_ROAD if "/Roads" in path else (SURFACE_TERRAIN if "/Terrain" in path else SURFACE_OTHER))
			else:
				wheel_ground_y[k].append(NAN)
				wheel_surface[k].append(SURFACE_AIR)
		var speed := car.linear_velocity.length()
		if float(state["throttle"]) > 0.0 and speed < STOP_SPEED_MS:
			stuck_ticks += 1
			if stuck_ticks == stop_ticks:
				stops += 1
				_report_stop(car, state, stops, float(i + 1) / tps)
		else:
			stuck_ticks = 0
	_release_all()
	var seconds := float(span_ticks) / tps
	var mean_y := 0.0
	for y in ys:
		mean_y += y
	mean_y /= maxf(ys.size(), 1)
	var var_y := 0.0
	for y in ys:
		var_y += (y - mean_y) * (y - mean_y)
	var stdev_y := sqrt(var_y / maxf(ys.size(), 1))
	var reached := int(state["index"]) >= waypoints.size() - 1
	print("drive bot: distance %.1f m in %.1f s (route %.1f m, end reached %s)" % [distance_m, seconds, route_m, str(reached)])
	print("drive bot: chassis y stdev %.4f m over %d ticks, end pos x=%.2f y=%.2f" % [stdev_y, ys.size(), car.global_position.x, -car.global_position.z])
	# Bounce only: residual of y from its centered 1 s moving mean, so the climb of the route does not count.
	var half := int(round(tps / 2.0))
	var resid_var := 0.0
	for i in ys.size():
		var lo := maxi(0, i - half)
		var hi := mini(ys.size(), i + half + 1)
		var avg := 0.0
		for j in range(lo, hi):
			avg += ys[j]
		avg /= float(hi - lo)
		resid_var += (ys[i] - avg) * (ys[i] - avg)
	print("drive bot: chassis y bounce stdev %.4f m (residual of a 1 s moving mean)" % sqrt(resid_var / maxf(ys.size(), 1)))
	print("drive bot: stops %d (speed < %.1f m/s with throttle for > %.1f s)" % [stops, STOP_SPEED_MS, STOP_HOLD_S])
	_report_bounce_origin(wheels, ys, wheel_local_y, wheel_ground_y, wheel_surface)
	await _finish(main, 0)


func _has_flag(name: String) -> bool:
	return ("--" + name) in OS.get_cmdline_user_args()


## Ram mode (item 89): settles, aims at the nearest building ahead, rams it at full throttle, reverses
## after the stop and prints the outcome. Returns the exit code: 0 when an outcome was reported, 1 when
## there was no building ahead.
func _run_ram(car: VehicleBody3D, opts: Dictionary) -> int:
	var tps := float(Engine.physics_ticks_per_second)
	for i in int(round(float(opts["settle"]) * tps)):
		await physics_frame
	var hit := {}
	for i in int(round(RAM_BUILDING_WAIT_S * tps)):
		hit = _building_ahead(car)
		if not hit.is_empty():
			break
		await physics_frame
	if hit.is_empty():
		push_error("drive bot: no building within %.0f m ahead of the car" % RAM_RAY_M)
		return 1
	var aim: Vector3 = hit["position"]
	var start := car.global_position
	print("drive bot: ram target %s at %.1f m ahead, aim x=%.2f y=%.2f z=%.2f" % [hit["collider"].get_path(), start.distance_to(aim), aim.x, aim.y, aim.z])

	# Contacts of the body are reported only for the bot, so the report can name what the car hit.
	car.contact_monitor = true
	car.max_contacts_reported = 16
	var cruise := float(opts["cruise"])
	var stop_ticks := int(round(STOP_HOLD_S * tps))
	var stuck_ticks := 0
	var through := false
	var impact := false
	var impact_t := 0.0
	var impact_pos := start
	var throttle := 0.0
	for i in int(round(RAM_APPROACH_MAX_S * tps)):
		throttle = 1.0 if car.linear_velocity.length() < cruise else 0.0
		_ram_steer(car, Vector2(aim.x, -aim.z), throttle)
		await physics_frame
		if _inside_building(car):
			through = true
			break
		if throttle > 0.0 and car.linear_velocity.length() < STOP_SPEED_MS:
			stuck_ticks += 1
			if stuck_ticks == stop_ticks:
				impact = true
				impact_t = float(i + 1) / tps
				impact_pos = car.global_position
				break
		else:
			stuck_ticks = 0
	if through or not impact:
		_release_all()
		if through:
			print("drive bot: ram outcome: through (the car went into the building, no stop)")
		else:
			print("drive bot: ram outcome: no impact within %.0f s" % RAM_APPROACH_MAX_S)
		return 0
	print("drive bot: impact at t=%.2f s, speed %.2f m/s, %.2f m from the car's start" % [impact_t, car.linear_velocity.length(), impact_pos.distance_to(start)])
	var bodies: PackedStringArray = []
	for body in car.get_colliding_bodies():
		bodies.append("%s (%s)" % [body.get_path(), body.get_class()])
	print("drive bot:   bodies: %s" % (", ".join(bodies) if not bodies.is_empty() else "none"))

	var clear := false
	var reverse_ticks := 0
	for i in int(round(RAM_REVERSE_MAX_S * tps)):
		_set_action("drive_forward", 0.0)
		_set_action("drive_back", 1.0)
		_set_action("drive_left", 0.0)
		_set_action("drive_right", 0.0)
		await physics_frame
		reverse_ticks = i + 1
		if _inside_building(car):
			through = true
			break
		if car.global_position.distance_to(impact_pos) >= RAM_CLEAR_M:
			clear = true
			break
	_release_all()
	var reverse_s := float(reverse_ticks) / tps
	var outcome := "through" if through else ("clear" if clear else "stuck")
	print("drive bot: reverse %.2f s, moved %.2f m from the impact" % [reverse_s, car.global_position.distance_to(impact_pos)])
	print("drive bot: ram outcome: %s" % outcome)
	return 0


## The nearest building hit by a ray from the car's front along its heading (empty when none within
## RAM_RAY_M). Hits on other bodies are skipped: the ray keeps going past them, up to RAM_RAY_SKIPS.
func _building_ahead(car: VehicleBody3D) -> Dictionary:
	var space := car.get_world_3d().direct_space_state
	var forward := -car.global_transform.basis.z
	var from := car.global_position + Vector3.UP * RAM_RAY_HEIGHT_M
	var exclude: Array[RID] = [car.get_rid()]
	for i in RAM_RAY_SKIPS:
		var query := PhysicsRayQueryParameters3D.create(from, from + forward * RAM_RAY_M, 0xFFFFFFFF, exclude)
		var hit := space.intersect_ray(query)
		if hit.is_empty():
			return {}
		if "/Buildings" in str((hit["collider"] as Node).get_path()):
			return hit
		exclude.append(hit["rid"])
	return {}


## True when the car's origin or one of its wheel centers is inside a building's collision shape.
func _inside_building(car: VehicleBody3D) -> bool:
	var space := car.get_world_3d().direct_space_state
	var points: Array[Vector3] = [car.global_position]
	for child in car.get_children():
		var wheel := child as VehicleWheel3D
		if wheel != null:
			points.append(wheel.global_position)
	for point in points:
		var query := PhysicsPointQueryParameters3D.new()
		query.position = point
		query.exclude = [car.get_rid()]
		for hit in space.intersect_point(query, 32):
			if "/Buildings" in str((hit["collider"] as Node).get_path()):
				return true
	return false


## Ram steering: pure pursuit toward a point (x, y in the route's 2D frame), throttle as given.
func _ram_steer(car: VehicleBody3D, aim: Vector2, throttle: float) -> void:
	var pos := car.global_position
	var p := Vector2(pos.x, -pos.z)
	var f := -car.global_transform.basis.z
	var fwd := Vector2(f.x, -f.z).normalized()
	var left := Vector2(-fwd.y, fwd.x)
	var d := aim - p
	var kappa := 2.0 * d.dot(left) / maxf(d.length_squared(), 1e-6)
	var speed := car.linear_velocity.length()
	var wheel_angle := atan(absf(car.get_node("FrontLeft").position.z - car.get_node("RearLeft").position.z) * kappa)
	var max_steer := float(car.get("max_steer"))
	var min_factor := float(car.get("min_steer_factor"))
	var full_speed := float(car.get("full_steer_speed"))
	var factor := lerpf(1.0, min_factor, clampf(speed / full_speed, 0.0, 1.0))
	var steer := clampf(wheel_angle / (max_steer * factor), -1.0, 1.0)
	_set_action("drive_forward", throttle)
	_set_action("drive_back", 0.0)
	_set_action("drive_left", maxf(steer, 0.0))
	_set_action("drive_right", maxf(-steer, 0.0))


## Frees the scene and quits on the next frame. Quitting with the world still alive let its background
## loaders (tile geometry, far lights) run while the engine and the extension were shutting down: exit 139.
## Freed first, their senders close and the world's Drop joins the terrain loader before the process ends.
func _finish(main: Node, code: int) -> void:
	main.queue_free()
	await process_frame
	quit(code)


## Probe only: shifts the route sideways by `offset` m (positive = left), easing in and out over
## 20 points so the start stays at the car. Used to drive onto the sidewalk on purpose.
## With `sweep`, the shift grows linearly from 0 at the start to `offset` at the end: the car crosses
## the road edge at an angle instead of parallel to it.
func _offset_route(route: PackedVector2Array, offset: float, sweep: bool) -> PackedVector2Array:
	if is_zero_approx(offset):
		return route
	var n := route.size()
	var shifted := PackedVector2Array()
	shifted.resize(n)
	for i in n:
		var prev := route[maxi(i - 1, 0)]
		var next := route[mini(i + 1, n - 1)]
		var tangent := (next - prev).normalized()
		var left := Vector2(-tangent.y, tangent.x)
		var ramp := float(i) / float(n - 1) if sweep else minf(1.0, minf(float(i) / 20.0, float(n - 1 - i) / 20.0))
		shifted[i] = route[i] + left * offset * ramp
	return shifted


## Logs one stop: position, route progress, and what each wheel and the body touch.
func _report_stop(car: VehicleBody3D, state: Dictionary, number: int, t_s: float) -> void:
	var pos := car.global_position
	var route: PackedVector2Array = state["route"]
	var index: int = state["index"]
	var wheels: PackedStringArray = []
	for child in car.get_children():
		var wheel := child as VehicleWheel3D
		if wheel == null:
			continue
		var contact := "air"
		if wheel.is_in_contact():
			var body := wheel.get_contact_body()
			contact = "%s (%s) at y=%.2f" % [body.get_path() if body else "?", body.get_class() if body else "?", wheel.get_contact_point().y]
		wheels.append("%s: %s" % [wheel.name, contact])
	var bodies: PackedStringArray = []
	for body in car.get_colliding_bodies():
		bodies.append("%s (%s)" % [body.get_path(), body.get_class()])
	print("drive bot: STOP %d at t=%.2f s, x=%.2f y=%.2f z=%.2f, speed %.2f m/s, route point %d of %d" % [number, t_s, pos.x, -pos.z, pos.y, car.linear_velocity.length(), index, route.size() - 1])
	print("drive bot:   wheels: %s" % ", ".join(wheels))
	print("drive bot:   bodies: %s" % (", ".join(bodies) if not bodies.is_empty() else "none"))


## Bounce of a series: stdev of its residual from a centered moving mean of `half` ticks on each side.
## NAN samples (wheel in the air) are skipped. With `mask` set, only the samples whose mask equals `want` count.
func _bounce_stdev(series: PackedFloat64Array, half: int, mask := PackedByteArray(), want := 0) -> float:
	var resid_var := 0.0
	var n := 0
	for i in series.size():
		if is_nan(series[i]) or (not mask.is_empty() and mask[i] != want):
			continue
		var avg := 0.0
		var m := 0
		for j in range(maxi(0, i - half), mini(series.size(), i + half + 1)):
			if not is_nan(series[j]):
				avg += series[j]
				m += 1
		avg /= float(m)
		resid_var += (series[i] - avg) * (series[i] - avg)
		n += 1
	return sqrt(resid_var / maxf(n, 1))


## Probe for the bounce origin: the chassis, the ground under the wheels (mean of the contact heights)
## and, per wheel, its suspension height and its contact heights split by what it touches.
func _report_bounce_origin(wheels: Array[VehicleWheel3D], ys: PackedFloat64Array, wheel_local_y: Array[PackedFloat64Array], wheel_ground_y: Array[PackedFloat64Array], wheel_surface: Array[PackedByteArray]) -> void:
	var tps := float(Engine.physics_ticks_per_second)
	var half := int(round(tps / 2.0))
	var ground := PackedFloat64Array()
	for i in ys.size():
		var sum := 0.0
		var m := 0
		for k in wheels.size():
			var g: float = wheel_ground_y[k][i]
			if not is_nan(g):
				sum += g
				m += 1
		ground.append(sum / float(m) if m > 0 else NAN)
	print("drive bot: bounce origin (1 s residual stdev): chassis %.4f m, ground under wheels %.4f m" % [_bounce_stdev(ys, half), _bounce_stdev(ground, half)])
	for k in wheels.size():
		var touch := [0, 0, 0, 0]
		for code in wheel_surface[k]:
			touch[code] += 1
		var n := maxf(ys.size(), 1)
		print("drive bot:   %s: air %.1f %%, terrain %.1f %%, road %.1f %%, other %.1f %%; suspension %.4f m; contact y %.4f m (on terrain %.4f m, on road %.4f m)" % [
			wheels[k].name,
			100.0 * touch[SURFACE_AIR] / n, 100.0 * touch[SURFACE_TERRAIN] / n, 100.0 * touch[SURFACE_ROAD] / n, 100.0 * touch[SURFACE_OTHER] / n,
			_bounce_stdev(wheel_local_y[k], half),
			_bounce_stdev(wheel_ground_y[k], half),
			_bounce_stdev(wheel_ground_y[k], half, wheel_surface[k], SURFACE_TERRAIN),
			_bounce_stdev(wheel_ground_y[k], half, wheel_surface[k], SURFACE_ROAD),
		])


## One control step: picks the route point ahead, steers toward it and sets the throttle.
func _steer_and_throttle(car: VehicleBody3D, state: Dictionary) -> void:
	var route: PackedVector2Array = state["route"]
	var cum: PackedFloat32Array = state["cum"]
	var speed := car.linear_velocity.length()
	var pos := car.global_position
	var p := Vector2(pos.x, -pos.z)
	var f := -car.global_transform.basis.z
	var fwd := Vector2(f.x, -f.z).normalized()
	var left := Vector2(-fwd.y, fwd.x)

	# Nearest route point, searched forward from the last one so the route is not re-walked.
	var index: int = state["index"]
	var best := index
	var best_d2 := INF
	for j in range(index, mini(index + SEARCH_AHEAD, route.size())):
		var d2 := p.distance_squared_to(route[j])
		if d2 < best_d2:
			best_d2 = d2
			best = j
	index = best
	state["index"] = index

	# Target: the first point at least `lookahead` meters further along the route.
	var lookahead := clampf(LOOKAHEAD_BASE_M + LOOKAHEAD_PER_MS * speed, LOOKAHEAD_MIN_M, LOOKAHEAD_MAX_M)
	var target := index
	while target < route.size() - 1 and cum[target] - cum[index] < lookahead:
		target += 1
	var d := route[target] - p
	var d_len2 := d.length_squared()
	var kappa := 0.0
	if d_len2 > 1e-6:
		kappa = 2.0 * d.dot(left) / d_len2

	# Pure pursuit steering angle, then the car's own steering scale (max_steer, speed factor).
	var wheel_angle := atan(float(state["wheelbase"]) * kappa)
	var max_steer := float(car.get("max_steer"))
	var min_factor := float(car.get("min_steer_factor"))
	var full_speed := float(car.get("full_steer_speed"))
	var factor := lerpf(1.0, min_factor, clampf(speed / full_speed, 0.0, 1.0))
	var steer := clampf(wheel_angle / (max_steer * factor), -1.0, 1.0)

	# Speed: the cruise, capped by the curve the route takes.
	var v_curve := sqrt(CURVE_LAT_ACCEL / maxf(absf(kappa), 1e-4))
	var v_target := minf(float(state["cruise"]), v_curve)
	var throttle := 1.0 if speed < v_target else 0.0
	if index >= route.size() - 1:
		throttle = 0.0

	state["throttle"] = throttle
	_set_action("drive_forward", throttle)
	_set_action("drive_back", 0.0)
	_set_action("drive_left", maxf(steer, 0.0))
	_set_action("drive_right", maxf(-steer, 0.0))


func _set_action(action: StringName, strength: float) -> void:
	if strength > 0.0:
		Input.action_press(action, strength)
	else:
		Input.action_release(action)


func _release_all() -> void:
	for action in ["drive_forward", "drive_back", "drive_left", "drive_right"]:
		Input.action_release(action)
