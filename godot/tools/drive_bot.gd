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

const LOOKAHEAD_MIN_M := 6.0
const LOOKAHEAD_MAX_M := 25.0
const LOOKAHEAD_PER_MS := 0.5
const LOOKAHEAD_BASE_M := 4.0
## Lateral acceleration the bot allows itself in curves, m/s².
const CURVE_LAT_ACCEL := 4.0
const SEARCH_AHEAD := 80


func _initialize() -> void:
	run()


func _parse_args() -> Dictionary:
	var opts := {
		"map": "casa-10km",
		"seconds": "20",
		"settle": "3",
		"length": "900",
		"cruise": "33.3",
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
		quit(1)
		return
	var car := cam.get_parent() as VehicleBody3D
	if car == null:
		push_error("drive bot: the camera is not under the car")
		quit(1)
		return

	var cache_root := ProjectSettings.globalize_path(str(world.get("cache_root")))
	var forward := -car.global_transform.basis.z
	var start := Vector2(car.global_position.x, -car.global_position.z)
	var heading := Vector2(forward.x, -forward.z).normalized()
	var length_m := float(opts["length"])
	var route_tool: Object = ClassDB.instantiate(&"DriveRoute")
	if route_tool == null:
		push_error("drive bot: class DriveRoute is not registered (extension not loaded?)")
		quit(1)
		return
	var waypoints: PackedVector2Array = route_tool.call("waypoints", cache_root, opts["map"], start, heading, length_m)
	if waypoints.size() < 2:
		push_error("drive bot: no route from the start point on map '%s'" % opts["map"])
		quit(1)
		return
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
	for i in span_ticks:
		_steer_and_throttle(car, state)
		await physics_frame
		var pos := car.global_position
		distance_m += Vector2(pos.x - prev_pos.x, pos.z - prev_pos.z).length()
		prev_pos = pos
		ys.append(pos.y)

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
	quit(0)


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
