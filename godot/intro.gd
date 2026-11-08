extends Control
## Intro shown on launch: the title for a few seconds, then main.tscn. Any key, button or Esc skips it.

const SHOW_SECONDS := 3.0
const MAIN_SCENE := "res://main.tscn"

var _elapsed := 0.0
var _leaving := false


func _process(delta: float) -> void:
	_elapsed += delta
	if _elapsed >= SHOW_SECONDS:
		_go_to_main()


func _input(event: InputEvent) -> void:
	if event is InputEventKey or event is InputEventMouseButton or event is InputEventJoypadButton:
		if event.is_pressed():
			_go_to_main()


func _go_to_main() -> void:
	if _leaving:
		return
	_leaving = true
	get_tree().change_scene_to_file(MAIN_SCENE)
