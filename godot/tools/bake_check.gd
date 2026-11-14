extends SceneTree

## Headless check of the first bake (item 78). With no map, the selector opens by itself. Confirm on
## a place with no baked map starts the bake and shows its progress. Cancel goes back to the selector
## with no map and no car. A second Confirm bakes the place, the new map becomes the last map, and the
## car ends on a road of it. Then a point about 100 m away, inside that map, is entered with no bake.
## Run with an empty user folder and a small radius, so the bake is short:
##   env -u DISPLAY -u WAYLAND_DISPLAY XDG_DATA_HOME=<empty dir> DTE_BAKE_RADIUS_M=400 \
##     godot --headless --path godot -s tools/bake_check.gd
## Needs the internet (Overpass and the Copernicus DEM). Exit code 0 when every check passes.
## The place is downtown Belo Horizonte; the map id is tilebake's id_for_coords of that point.

const PLACE_LAT := -19.92
const PLACE_LON := -43.94
## About 100 m north of the place: inside the 400 m map, so it is a place already baked.
const NEAR_LAT := -19.9191
const EXPECTED_ID := "at_m19p9200_m43p9400"
const SELECTOR_WAIT_S := 60.0
const BAKE_START_WAIT_S := 120.0
const BAKE_STOP_WAIT_S := 120.0
const BAKE_WAIT_S := 1500.0
const LAST_MAP_PATH := "user://last_map.txt"

var _progress_events := 0
var _failed := 0


func _initialize() -> void:
	run()


func run() -> void:
	var main: Node = (load("res://main.tscn") as PackedScene).instantiate()
	root.add_child(main)
	var world: Node = main.get_node("World")
	await _wait_until(func(): return _find_picker(root) != null, SELECTOR_WAIT_S)
	var picker = _find_picker(root)
	if picker == null:
		_check("selector opens by itself with no map", false)
		await _finish(main)
		return
	var bake = null
	for child in picker.get_children():
		if child.has_method("is_running"):
			bake = child
	_check("bake node is under the selector", bake != null)
	if bake == null:
		await _finish(main)
		return
	bake.progress.connect(_on_progress)
	bake.failed.connect(func(kind, message): print("bake_check: bake failed (%s): %s" % [kind, message]))

	_check("selector opens by itself with no map", picker.visible and paused)
	_check("no car before a map", world.find_child("Car", true, false) == null)
	_check("no last map before a bake", not FileAccess.file_exists(LAST_MAP_PATH))

	# Cancel once, through the Cancel button, while the bake runs.
	picker.confirm_place(PLACE_LAT, PLACE_LON)
	var started := await _wait_until(
		func(): return bake.is_running() and _progress_events > 0, BAKE_START_WAIT_S)
	_check("bake starts with progress", started)
	var cancel := _find_cancel(picker)
	_check("progress box with Cancel is shown while baking",
		cancel != null and cancel.is_visible_in_tree())
	if cancel != null:
		cancel.pressed.emit()
	var stopped := await _wait_until(func(): return not bake.is_running(), BAKE_STOP_WAIT_S)
	_check("Cancel stops the bake", stopped)
	_check("Cancel goes back to the selector",
		picker.visible and paused and world.find_child("Car", true, false) == null)
	_check("Cancel hides the progress box", cancel != null and not cancel.is_visible_in_tree())
	_check("Cancel leaves no last map", not FileAccess.file_exists(LAST_MAP_PATH))

	# Confirm again: the bake runs to the end, and the game opens on the new map.
	_progress_events = 0
	picker.confirm_place(PLACE_LAT, PLACE_LON)
	var entered := await _wait_until(func(): return _car(world) != null and not paused, BAKE_WAIT_S)
	_check("bake finishes and the game opens", entered)
	_check("progress was reported", _progress_events > 0)
	_check("selector closes when the car is on the new map", not picker.visible)
	await _frames(30)
	var car := _car(world)
	_check("last map is the baked place",
		FileAccess.file_exists(LAST_MAP_PATH)
		and FileAccess.get_file_as_string(LAST_MAP_PATH).strip_edges() == EXPECTED_ID)
	_check("car is on a road of the new map", car != null and _on_road(car))

	# A place already baked is entered with no bake.
	var progress_before := _progress_events
	var position_before: Vector3 = car.global_position
	picker.confirm_place(NEAR_LAT, PLACE_LON)
	await _frames(30)
	var moved := car.global_position.distance_to(position_before)
	_check("place already baked: no bake", _progress_events == progress_before and not bake.is_running())
	_check("place already baked: the car moves on its road (%.0f m)" % moved,
		moved > 10.0 and _on_road(car))

	print("bake_check: %s" % ("all checks passed" if _failed == 0 else "%d check(s) failed" % _failed))
	await _finish(main)


func _on_progress(stage: String, done: int, total: int) -> void:
	_progress_events += 1
	print("bake_check: progress %s %d/%d" % [stage, done, total])


func _check(name: String, ok: bool) -> void:
	print("bake_check: %s %s" % ["PASS" if ok else "FAIL", name])
	if not ok:
		_failed += 1


func _find_picker(from: Node) -> Node:
	for node in from.find_children("*", "", true, false):
		if node.has_method("confirm_place"):
			return node
	return null


func _find_cancel(picker: Node) -> Button:
	for node in picker.find_children("*", "Button", true, false):
		var button := node as Button
		if button != null and (button.text == "Cancel" or button.text == "Cancelar"):
			return button
	return null


func _car(world: Node) -> Node3D:
	return world.find_child("Car", true, false) as Node3D


## True when the car stands on the solid road body of its tile (the ray drops from above the car
## and skips the car itself).
func _on_road(car: Node3D) -> bool:
	var from := car.global_position + Vector3.UP * 2.0
	var query := PhysicsRayQueryParameters3D.create(from, from + Vector3.DOWN * 8.0)
	query.exclude = [car.get_rid()]
	var hit := car.get_world_3d().direct_space_state.intersect_ray(query)
	return not hit.is_empty() and hit.collider.name == "Roads"


func _wait_until(condition: Callable, timeout_s: float) -> bool:
	var deadline := Time.get_ticks_msec() + int(timeout_s * 1000.0)
	while Time.get_ticks_msec() < deadline:
		await process_frame
		if condition.call():
			return true
	return false


func _frames(count: int) -> void:
	for i in count:
		await process_frame


func _finish(main: Node) -> void:
	main.queue_free()
	await process_frame
	quit(0 if _failed == 0 else 1)
