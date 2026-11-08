extends SceneTree

## GPS check (item 93): a bot drives to a destination about 1 km away by car and steers only by the GPS.
## Each physics tick it feeds the car position to Gps.update(), reads Gps.guide_direction() and
## steers toward it, with a speed target that eases off in bends and near the end. It prints the
## route, the time to arrival, the distance left, and the cost of Gps.update() per tick.
## Run:
##   env -u DISPLAY -u WAYLAND_DISPLAY godot --headless --path godot -s tools/gps_check.gd -- --map casa-10km
## Optional: --length 1000 (road distance from the spawn where the candidates start, m), --limit 300
## (simulated s allowed to arrive).
## Exit code 0 when the car arrives (signal `arrived`), 1 otherwise.
##
## Destination: the car route length (Gps.remaining_m) is the one that counts as "about 1 km", not the
## road length: a road route of 1000 m may have a much shorter car route to its end. Candidates are
## points of a DriveRoute, every CANDIDATE_STEP_M of road; the first one whose car route is
## DEST_MIN_M..DEST_MAX_M is used.
## Route: Gps (crates/gdext/src/world/gps.rs), the car's road route to the destination; the bot only
## follows it. Cost: time of Gps.update() on the physics thread, in ms (the route search runs in a
## thread of its own and is not in this number).

const TARGET_SPEED_MS := 16.0
const MIN_SPEED_MS := 4.0
## Speed target loss per radian of heading error, so the bot slows for bends.
const BEND_SPEED_LOSS_MS := 25.0
## Speed target near the end of the route (m) and the speed there (m/s).
const END_ZONE_M := 40.0
const END_SPEED_MS := 4.0
## Steering: heading error (rad) at full lock is 1 / STEER_GAIN, and how hard the car brakes.
const STEER_GAIN := 1.5
const BRAKE_GAIN := 0.5
## Speed under which the car is not braked, to keep it from reversing.
const BRAKE_FLOOR_MS := 2.0
## Destination candidates: the road distance they span from the spawn and their step (m), and the
## car-route length they must have (m).
const CANDIDATE_SPAN_M := 3000.0
const CANDIDATE_STEP_M := 50.0
const DEST_MIN_M := 950.0
const DEST_MAX_M := 1050.0
## Physics ticks given to a route search before a candidate is given up.
const SEARCH_LIMIT_TICKS := 600
## Cost limit of Gps.update() on the physics thread (item 93 acceptance), in ms.
const COST_LIMIT_MS := 0.2
## Time to wait for the graph to load, in real s (the load is about 3 s in release).
const LOAD_LIMIT_S := 120.0

var _arrived := false


func _initialize() -> void:
	run()


func _parse_args() -> Dictionary:
	var opts := {
		"map": "casa-10km",
		"length": "1000",
		"limit": "300",
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
		push_error("gps check: no car camera under World (spawn failed?)")
		await _finish(main, 1)
		return
	var car := cam.get_parent() as VehicleBody3D
	if car == null:
		push_error("gps check: the camera is not under the car")
		await _finish(main, 1)
		return

	var cache_root := ProjectSettings.globalize_path(str(world.get("cache_root")))
	var forward := -car.global_transform.basis.z
	var start := Vector2(car.global_position.x, -car.global_position.z)
	var heading := Vector2(forward.x, -forward.z).normalized()
	var first_length := float(opts["length"])

	var route_tool: Object = ClassDB.instantiate(&"DriveRoute")
	if route_tool == null:
		push_error("gps check: class DriveRoute is not registered (extension not loaded?)")
		await _finish(main, 1)
		return
	var gps: Object = ClassDB.instantiate(&"Gps")
	if gps == null:
		push_error("gps check: class Gps is not registered (extension not loaded?)")
		await _finish(main, 1)
		return
	gps.connect("arrived", _on_arrived)
	gps.call("load_map", cache_root, opts["map"])
	var load_t0 := Time.get_ticks_msec()
	while not gps.call("is_loaded"):
		await process_frame
		if Time.get_ticks_msec() - load_t0 > LOAD_LIMIT_S * 1000.0:
			push_error("gps check: the map '%s' did not load in %.0f s" % [opts["map"], LOAD_LIMIT_S])
			await _finish(main, 1)
			return
	print("gps check: map %s loaded in %.2f s" % [opts["map"], (Time.get_ticks_msec() - load_t0) / 1000.0])

	# Pick the destination by the GPS's own car route length. Candidates are points of one long
	# DriveRoute from the spawn, taken every CANDIDATE_STEP_M of road, in order.
	var dest := Vector2.ZERO
	var picked := false
	var fallback := Vector2.ZERO
	var fallback_gap := INF
	var full: PackedVector2Array = route_tool.call("waypoints", cache_root, opts["map"], start, heading, CANDIDATE_SPAN_M)
	if full.size() < 2:
		push_error("gps check: no road route from the start point on map '%s'" % opts["map"])
		await _finish(main, 1)
		return
	var cum := PackedFloat64Array([0.0])
	for i in range(1, full.size()):
		cum.append(cum[i - 1] + full[i].distance_to(full[i - 1]))
	var cand_index := 0
	var at_m := first_length
	while not picked and at_m <= CANDIDATE_SPAN_M:
		while cand_index < full.size() - 1 and cum[cand_index] < at_m:
			cand_index += 1
		var road_at_m := at_m
		at_m += CANDIDATE_STEP_M
		if cand_index < 1:
			continue
		var cand: Vector2 = full[cand_index]
		gps.call("set_destination", cand.x, cand.y)
		var tries := 0
		while not gps.call("has_route") and tries < SEARCH_LIMIT_TICKS:
			await physics_frame
			tries += 1
			var p := car.global_position
			gps.call("update", p.x, -p.z)
		if not gps.call("has_route"):
			print("gps check: candidate %.0f m of road: no car route" % road_at_m)
			continue
		var car_route_m: float = gps.call("remaining_m")
		print("gps check: candidate %.0f m of road: car route %.1f m" % [road_at_m, car_route_m])
		if car_route_m >= DEST_MIN_M and car_route_m <= DEST_MAX_M:
			dest = cand
			picked = true
			break
		if absf(car_route_m - 1000.0) < fallback_gap:
			fallback_gap = absf(car_route_m - 1000.0)
			fallback = cand
	if not picked:
		if fallback_gap == INF:
			push_error("gps check: no candidate with a car route from the start point")
			await _finish(main, 1)
			return
		push_warning("gps check: no candidate in %.0f..%.0f m; using the closest one" % [DEST_MIN_M, DEST_MAX_M])
		dest = fallback

	gps.call("set_destination", dest.x, dest.y)
	print("gps check: start x=%.2f y=%.2f, destination x=%.2f y=%.2f, straight line %.1f m" % [
		start.x, start.y, dest.x, dest.y, start.distance_to(dest)])
	var tps := float(Engine.physics_ticks_per_second)
	var max_ticks := int(round(float(opts["limit"]) * tps))
	var costs := PackedFloat64Array()
	var ticks := 0
	var driven_m := 0.0
	var prev := car.global_position
	var remaining_start := -1.0
	var had_route := false
	var lost_route_ticks := 0
	var max_speed := 0.0
	while not _arrived and ticks < max_ticks:
		await physics_frame
		ticks += 1
		var pos := car.global_position
		driven_m += Vector2(pos.x - prev.x, pos.z - prev.z).length()
		prev = pos
		var mp := Vector2(pos.x, -pos.z)

		var c0 := Time.get_ticks_usec()
		gps.call("update", mp.x, mp.y)
		costs.append((Time.get_ticks_usec() - c0) / 1000.0)

		forward = -car.global_transform.basis.z
		var fwd := Vector2(forward.x, -forward.z).normalized()
		var has: bool = gps.call("has_route")
		if has:
			had_route = true
			if remaining_start < 0.0:
				remaining_start = float(gps.call("remaining_m"))
		else:
			lost_route_ticks += 1
		var steer := 0.0
		var throttle := 0.0
		var speed := car.linear_velocity.length()
		max_speed = maxf(max_speed, speed)
		if has:
			var guide: Vector2 = gps.call("guide_direction")
			var err := fwd.angle_to(guide)
			steer = clampf(err * STEER_GAIN, -1.0, 1.0)
			var target := maxf(TARGET_SPEED_MS - BEND_SPEED_LOSS_MS * absf(err), MIN_SPEED_MS)
			var remaining := float(gps.call("remaining_m"))
			if remaining < END_ZONE_M:
				target = minf(target, END_SPEED_MS)
			if speed < target:
				throttle = clampf((target - speed) * 0.5, 0.0, 1.0)
			elif speed > target + BRAKE_FLOOR_MS:
				throttle = -clampf((speed - target) * BRAKE_GAIN, 0.0, 1.0)
		_set_drive(throttle, steer)
		if ticks % int(tps) == 0:
			var g: Vector2 = gps.call("guide_direction")
			print("gps trace: t=%.0f s pos=(%.1f, %.1f) speed=%.1f m/s fwd=(%.2f, %.2f) guide=(%.2f, %.2f) steer=%.2f throttle=%.2f rem=%.1f has=%s" % [
				float(ticks) / tps, mp.x, mp.y, speed, fwd.x, fwd.y, g.x, g.y, steer, throttle,
				float(gps.call("remaining_m")), str(has)])
	_release_all()

	var end_xz := Vector2(car.global_position.x, -car.global_position.z)
	var end_dist := end_xz.distance_to(dest)
	var sim_s := float(ticks) / tps
	print("gps check: %s after %d ticks (%.1f simulated s), driven %.1f m, end %.1f m from the destination" % [
		"ARRIVED" if _arrived else "NOT ARRIVED", ticks, sim_s, driven_m, end_dist])
	print("gps check: car route at start %.1f m, had_route %s, ticks without a route %d, max speed %.1f m/s" % [
		remaining_start, str(had_route), lost_route_ticks, max_speed])
	costs.sort()
	var n := costs.size()
	if n > 0:
		var total := 0.0
		for c in costs:
			total += c
		var p95 := costs[mini(n - 1, int(floor(0.95 * (n - 1))))]
		var cost_max := costs[n - 1]
		print("gps check: Gps.update() cost over %d ticks: mean %.4f ms, p95 %.4f ms, max %.4f ms (limit %.2f ms)" % [
			n, total / n, p95, cost_max, COST_LIMIT_MS])
	gps.free()
	await _finish(main, 0 if _arrived else 1)


func _on_arrived() -> void:
	_arrived = true
	print("gps check: signal arrived")


func _set_drive(throttle: float, steer: float) -> void:
	_set_action("drive_forward", maxf(throttle, 0.0))
	_set_action("drive_back", maxf(-throttle, 0.0))
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


func _finish(main: Node, code: int) -> void:
	_release_all()
	main.queue_free()
	await process_frame
	quit(code)
