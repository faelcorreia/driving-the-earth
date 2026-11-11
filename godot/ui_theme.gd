extends Node
## Applies the active visual theme to the Godot UI (item 122), as the `UiTheme` autoload.
##
## Only the UI fields of `Style` are read: panel, ink, accent, border, border_width and
## corner_radius. The theme is installed on the root window once, so every Control under it
## (buttons, panels, labels) inherits it. The speed HUD is a CanvasLayer outside that Control
## tree, so its nodes are styled directly after the scene is ready.

const HUD_GROUP := "speed_hud"


func _ready() -> void:
	get_tree().root.theme = _build_theme()
	get_tree().node_added.connect(_on_node_added)
	_style_speed_hud.call_deferred()


func _on_node_added(node: Node) -> void:
	if node.is_in_group(HUD_GROUP):
		_style_speed_hud.call_deferred()


func _build_theme() -> Theme:
	var panel := Style.color("ui", "panel")
	var ink := Style.color("ui", "ink")
	var accent := Style.color("ui", "accent")
	var border := Style.color("ui", "border")
	var width := int(Style.number("ui", "border_width", 0.0))
	var radius := int(Style.number("ui", "corner_radius", 0.0))
	var focus_width := maxi(width, 2)
	var theme := Theme.new()

	theme.set_color("font_color", "Label", ink)

	# OptionButton is a Button subclass but has its own theme type; style both.
	for kind in ["Button", "OptionButton"]:
		theme.set_color("font_color", kind, ink)
		theme.set_color("font_hover_color", kind, ink)
		theme.set_color("font_focus_color", kind, ink)
		theme.set_color("font_pressed_color", kind, ink)
		theme.set_color("font_disabled_color", kind, _alpha(ink, 0.55))
		theme.set_color("icon_normal_color", kind, ink)
		theme.set_color("icon_hover_color", kind, ink)
		theme.set_color("icon_focus_color", kind, ink)
		theme.set_color("icon_pressed_color", kind, ink)
		theme.set_color("icon_disabled_color", kind, _alpha(ink, 0.55))
		theme.set_stylebox("normal", kind, _box(panel, border, width, radius))
		theme.set_stylebox("hover", kind, _box(_alpha(accent, 0.16), accent, width, radius))
		theme.set_stylebox("pressed", kind, _box(_alpha(accent, 0.28), accent, width, radius))
		theme.set_stylebox("focus", kind, _box(_alpha(panel, 0.0), accent, focus_width, radius))
		theme.set_stylebox("disabled", kind, _box(_alpha(panel, 0.55), _alpha(border, 0.45), width, radius))

	for kind in ["Panel", "PanelContainer"]:
		theme.set_stylebox("panel", kind, _box(panel, border, width, radius))

	# The time panel's speed selector opens a PopupMenu; it must not fall back to the engine theme.
	theme.set_color("font_color", "PopupMenu", ink)
	theme.set_color("font_hover_color", "PopupMenu", ink)
	theme.set_color("font_disabled_color", "PopupMenu", _alpha(ink, 0.55))
	theme.set_stylebox("panel", "PopupMenu", _box(panel, border, width, radius))
	theme.set_stylebox("hover", "PopupMenu", _box(_alpha(accent, 0.22), accent, width, radius))
	return theme


## Rounded panel, accent rule and text of the speed readout.
func _style_speed_hud() -> void:
	var panel := Style.color("ui", "panel")
	var ink := Style.color("ui", "ink")
	var accent := Style.color("ui", "accent")
	var border := Style.color("ui", "border")
	var width := int(Style.number("ui", "border_width", 0.0))
	var radius := int(Style.number("ui", "corner_radius", 0.0))
	for hud in get_tree().get_nodes_in_group(HUD_GROUP):
		var backing := hud.find_child("Backing", true, false)
		if backing is Panel:
			(backing as Panel).add_theme_stylebox_override(
				"panel", _box(panel, border, width, radius)
			)
		var rule := hud.find_child("Accent", true, false)
		if rule is ColorRect:
			(rule as ColorRect).color = accent
			(rule as ColorRect).visible = true
		var value := hud.find_child("Value", true, false)
		if value is Label:
			(value as Label).add_theme_color_override("font_color", ink)
			# The speed value keeps ink for the 4.5:1 text contrast, with an accent drop shadow.
			(value as Label).add_theme_color_override("font_shadow_color", accent)
			(value as Label).add_theme_constant_override("shadow_offset_x", 2)
			(value as Label).add_theme_constant_override("shadow_offset_y", 2)
		var unit := hud.find_child("Unit", true, false)
		if unit is Label:
			(unit as Label).add_theme_color_override("font_color", ink)


## StyleBoxFlat of `bg` and `line`, with the theme border width and corner radius.
func _box(bg: Color, line: Color, width: int, radius: int) -> StyleBoxFlat:
	var box := StyleBoxFlat.new()
	box.bg_color = bg
	box.border_color = line
	box.set_border_width_all(width)
	box.set_corner_radius_all(radius)
	return box


## The same color with another alpha (transparency only; the hue still comes from the theme).
func _alpha(color: Color, alpha: float) -> Color:
	return Color(color.r, color.g, color.b, alpha)
