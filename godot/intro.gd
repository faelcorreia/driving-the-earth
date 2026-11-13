extends Control
## Intro shown on launch: the title for a few seconds, then main.tscn. Any key, mouse button or
## joypad button skips it once the guard below has passed.

const SHOW_SECONDS := 3.0
## Input in the first moments is ignored: a click that only focuses the window, or a key/joypad
## button still held from the launch, would skip the intro before it is drawn (and before the hint
## can be read). It is a tenth of the automatic skip, so a player who wants to skip loses nothing.
const INPUT_GUARD_SECONDS := 0.3
const MAIN_SCENE := "res://main.tscn"

var _elapsed := 0.0
var _leaving := false


func _ready() -> void:
	_apply_theme()


## Dark backdrop, light rounded card, ink title and dimmed hint, all from the active UI theme.
func _apply_theme() -> void:
	var panel := Style.color("ui", "panel")
	var ink := Style.color("ui", "ink")
	var accent := Style.color("ui", "accent")
	var border := Style.color("ui", "border")
	var width := int(Style.number("ui", "border_width", 0.0))
	var radius := int(Style.number("ui", "corner_radius", 0.0))
	($Background as ColorRect).color = ink
	($Card as Panel).add_theme_stylebox_override("panel", _box(panel, border, width, radius))
	($Card/Accent as ColorRect).color = accent
	($Title as Label).add_theme_color_override("font_color", ink)
	($Card/Hint as Label).add_theme_color_override("font_color", Color(ink.r, ink.g, ink.b, 0.8))


func _box(bg: Color, line: Color, width: int, radius: int) -> StyleBoxFlat:
	var box := StyleBoxFlat.new()
	box.bg_color = bg
	box.border_color = line
	box.set_border_width_all(width)
	box.set_corner_radius_all(radius)
	return box


func _process(delta: float) -> void:
	_elapsed += delta
	if _elapsed >= SHOW_SECONDS:
		_go_to_main()


func _input(event: InputEvent) -> void:
	if _elapsed < INPUT_GUARD_SECONDS:
		return
	if event is InputEventKey or event is InputEventMouseButton or event is InputEventJoypadButton:
		if event.is_pressed():
			_go_to_main()


func _go_to_main() -> void:
	if _leaving:
		return
	_leaving = true
	get_tree().change_scene_to_file(MAIN_SCENE)
