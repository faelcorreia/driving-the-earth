extends SceneTree
## Headless check of the intro's skip paths (item 131). One run per input kind:
##   godot --headless --path godot -s tools/intro_skip.gd -- --input keyboard
## `--input` is `keyboard`, `mouse`, `joypad` or `none` (sends nothing, for the automatic skip);
## `--expect` is `skip` (default: the intro hands over to main.tscn) or `stay` (default wait 0.0:
## the event lands in the 0.3 s guard and the intro must stay). `--wait` overrides the seconds
## before the event (default 0.5, past the guard). Exit codes: 0 on the expected result, 1
## otherwise, 2 for a bad argument.
##
## The scene is installed as `current_scene`, like on launch, so the check sees the real
## `change_scene_to_file("res://main.tscn")` of the intro.

const INTRO_SCENE := "res://intro.tscn"
const MAIN_SCENE := "res://main.tscn"
## How long to wait for the scene change; below the 3 s automatic skip of the intro.
const TIMEOUT_SECONDS := 2.0


func _initialize() -> void:
	run()


func _parse_args() -> Dictionary:
	var opts := {
		"input": "keyboard",
		"expect": "skip",
		"wait": -1.0,
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


## One event of the requested kind, the same shape the player produces.
func _make_event(kind: String) -> InputEvent:
	match kind:
		"keyboard":
			var key := InputEventKey.new()
			key.physical_keycode = KEY_ENTER
			key.keycode = KEY_ENTER
			key.pressed = true
			return key
		"mouse":
			var click := InputEventMouseButton.new()
			click.button_index = MOUSE_BUTTON_LEFT
			click.pressed = true
			click.position = Vector2(root.size) * 0.5
			return click
		"joypad":
			var button := InputEventJoypadButton.new()
			button.device = 0
			button.button_index = JOY_BUTTON_A
			button.pressed = true
			return button
	return null


func _wait_seconds(seconds: float) -> void:
	var started := Time.get_ticks_usec()
	while Time.get_ticks_usec() - started < int(seconds * 1000000.0):
		await process_frame


func _seconds_since(started: int) -> float:
	return float(Time.get_ticks_usec() - started) / 1000000.0


## Waits at most `seconds` for the current scene to become main.tscn.
func _wait_for_main(seconds: float) -> bool:
	var started := Time.get_ticks_usec()
	while Time.get_ticks_usec() - started < int(seconds * 1000000.0):
		await process_frame
		if current_scene != null and current_scene.scene_file_path == MAIN_SCENE:
			return true
	return false


func run() -> void:
	var opts := _parse_args()
	var event := _make_event(opts["input"])
	if event == null and opts["input"] != "none":
		push_error("intro_skip: unknown --input %s" % opts["input"])
		quit(2)
		return
	var expect: String = opts["expect"]
	var wait := float(opts["wait"])
	if wait < 0.0:
		wait = 0.0 if expect == "stay" else 0.5
	if expect != "skip" and expect != "stay":
		push_error("intro_skip: unknown --expect %s" % expect)
		quit(2)
		return

	var intro: Node = (load(INTRO_SCENE) as PackedScene).instantiate()
	root.add_child(intro)
	current_scene = intro
	await process_frame
	var hint := intro.get_node_or_null("Card/Hint") as Label
	if hint == null or hint.text.is_empty():
		push_error("intro_skip: the intro has no hint line")
		quit(1)
		return
	print("intro_skip: locale '%s', hint '%s'" % [OS.get_locale(), hint.text])
	var started := Time.get_ticks_usec()
	await _wait_seconds(wait)
	if event != null:
		Input.parse_input_event(event)

	if expect == "skip":
		# Without an event the intro must still hand over on its own, at SHOW_SECONDS (3 s).
		var timeout := 4.0 if event == null else TIMEOUT_SECONDS
		if await _wait_for_main(timeout):
			var when := "no input" if event == null else "event at %.2f s" % wait
			print("intro_skip: %s, %s, main.tscn after %.2f s" % [
				opts["input"], when, _seconds_since(started)])
			quit(0)
		else:
			push_error("intro_skip: %s at %.2f s did not reach main.tscn" % [opts["input"], wait])
			quit(1)
		return

	await _wait_seconds(0.5)
	var still_intro := current_scene != null and current_scene.scene_file_path == INTRO_SCENE
	if still_intro:
		print("intro_skip: %s during the guard -> still on the intro" % opts["input"])
		quit(0)
	else:
		push_error("intro_skip: %s during the guard skipped the intro" % opts["input"])
		quit(1)
