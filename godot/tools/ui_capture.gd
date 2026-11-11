extends SceneTree
## UI capture of the themed screens (item 122). Saves one PNG per screen:
##   intro.png, speed_hud.png, time_panel.png, world_map.png.
##
## Usage (inside the no-window sandbox; a real renderer is needed for pixels):
##   godot-capture-sandbox.sh --path godot -s tools/ui_capture.gd -- --out <dir> --style lowpoly
## The active theme comes from `--style` through the `Style` autoload, like the game.
## `--size 1280x720` (default) or `--size 1920x1080`.
## Under `--headless` the script still runs the whole flow but skips the PNGs, which have no pixels.

const DEFAULT_SIZE := Vector2i(1280, 720)
const INTRO_FRAMES := 6
const WORLD_FRAMES := 240
const SETTLE_FRAMES := 30
const MAP_FRAMES := 90


func _initialize() -> void:
	run()


func _parse_args() -> Dictionary:
	var opts := {
		"out": "",
		"size": "1280x720",
	}
	var args := OS.get_cmdline_user_args()
	var i := 0
	while i < args.size():
		var key := String(args[i]).trim_prefix("--").replace("-", "_")
		if opts.has(key) and i + 1 < args.size():
			opts[key] = args[i + 1]
			i += 2
		else:
			i += 1
	return opts


func _parse_size(text: String) -> Vector2i:
	var parts := text.split("x", false)
	if parts.size() != 2:
		return DEFAULT_SIZE
	return Vector2i(maxi(1, parts[0].to_int()), maxi(1, parts[1].to_int()))


## Sends an action event, so the game's own shortcut handlers open/close the panels.
func _send_action(action: String) -> void:
	var event := InputEventAction.new()
	event.action = action
	event.pressed = true
	Input.parse_input_event(event)


func _snap(path: String) -> void:
	if DisplayServer.get_name() == "headless":
		print("ui_capture: headless, skipping ", path)
		return
	await RenderingServer.frame_post_draw
	var texture := root.get_texture()
	if texture == null:
		push_warning("ui_capture: no viewport texture for %s" % path)
		return
	var image := texture.get_image()
	if image == null or image.is_empty():
		push_warning("ui_capture: empty image for %s" % path)
		return
	var err := image.save_png(path)
	if err != OK:
		push_error("ui_capture: cannot save %s (error %d)" % [path, err])
	else:
		print("ui_capture: saved ", path)


func run() -> void:
	var opts := _parse_args()
	var out_dir: String = opts["out"]
	if out_dir == "":
		push_error("ui_capture: --out is required")
		quit(2)
		return
	DirAccess.make_dir_recursive_absolute(out_dir)
	var size := _parse_size(opts["size"])
	DisplayServer.window_set_size(size)
	root.size = size
	print("ui_capture: size %s" % size)

	# Intro, before the 3 s timer hands over to the main scene.
	var intro: Node = (load("res://intro.tscn") as PackedScene).instantiate()
	root.add_child(intro)
	for i in INTRO_FRAMES:
		await process_frame
	await _snap(out_dir.path_join("intro.png"))
	intro.queue_free()
	for i in 4:
		await process_frame

	# Main scene: the world, the car and the clock.
	var main: Node = (load("res://main.tscn") as PackedScene).instantiate()
	var world := main.get_node("World")
	root.add_child(main)
	var clock: Node = null
	var car: Node3D = null
	for i in WORLD_FRAMES:
		await process_frame
		clock = world.find_child("Environment", true, false)
		var camera := world.find_child("Camera", true, false) as Camera3D
		if clock != null and camera != null:
			car = camera.get_parent() as Node3D
			break
	if clock == null or car == null:
		push_error("ui_capture: the world, the car or the clock did not load")
		quit(1)
		return
	for i in SETTLE_FRAMES:
		await process_frame
	await _snap(out_dir.path_join("speed_hud.png"))

	# Time panel (T), themed in Rust.
	_send_action("env_time_panel")
	for i in 20:
		await process_frame
	await _snap(out_dir.path_join("time_panel.png"))
	_send_action("env_time_panel")
	for i in 4:
		await process_frame

	# World map (M): the game pauses, the map keeps processing.
	_send_action("map_toggle")
	for i in MAP_FRAMES:
		await process_frame
	await _snap(out_dir.path_join("world_map.png"))
	# Close the map again so the tree is unpaused when the scene is torn down.
	_send_action("map_toggle")
	for i in 4:
		await process_frame
	# Let the terrain loader threads finish before the scene is freed and the engine quits.
	main.queue_free()
	for i in 60:
		await process_frame

	print("ui_capture: done in ", out_dir)
	quit(0)
