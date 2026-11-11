extends CanvasLayer
## Loading screen shown by `EarthWorld` while the first frame is prepared (item 123).
##
## The screen never reads the cache or the map: Rust drives it through `set_stage` (one call per
## real stage and per tile) and closes it through `finish`. Colors and corners come from the
## `Style` autoload (item 118); without it the built-in lowpoly values are used, so the screen
## still shows up. The whole tree ignores the mouse, so it can never eat a click or a key.

## English fallbacks for the few keys the screen uses when a translation is missing.
const FALLBACK := {
	"intro.title": "Driving the Earth",
	"loading.title": "Loading",
	"loading.opening_map": "Opening the map",
	"loading.reading_map": "Reading the map",
	"loading.building_tiles": "Building the first blocks",
	"loading.placing_car": "Placing the car",
	"loading.environment": "Turning on the lights",
	"loading.ready": "Ready",
	"loading.hint": "The first blocks load around the car.",
	"loading.error": "Could not load this map",
}

## Built-in lowpoly UI colors, used when the `Style` autoload is missing or broken.
const DEFAULT_PANEL := Color(0.9568627, 0.9372549, 0.8941177, 1.0)
const DEFAULT_INK := Color(0.1803922, 0.1647059, 0.1490196, 1.0)
const DEFAULT_ACCENT := Color(0.8509804, 0.454902, 0.2901961, 1.0)
const DEFAULT_BORDER := Color(0.5490196, 0.4980392, 0.4196078, 1.0)

var _screen: Control
var _card: PanelContainer
var _title: Label
var _stage: Label
var _hint: Label
var _progress: ProgressBar
var _finished := false


func _ready() -> void:
	_screen = get_node_or_null("Screen") as Control
	_card = get_node_or_null("Screen/Card") as PanelContainer
	_title = get_node_or_null("Screen/Card/Margin/Column/Title") as Label
	_stage = get_node_or_null("Screen/Card/Margin/Column/Stage") as Label
	_hint = get_node_or_null("Screen/Card/Margin/Column/Hint") as Label
	_progress = get_node_or_null("Screen/Card/Margin/Column/Progress") as ProgressBar
	_apply_theme()
	if _title != null:
		_title.text = _tr("intro.title")
	if _hint != null:
		_hint.text = _tr("loading.hint")
	set_stage("loading.opening_map", 0.0)


## Title of the card, as an i18n key or an already translated line.
func set_title(text_key_or_text: String) -> void:
	if _finished:
		return
	if _title != null:
		_title.text = _tr(text_key_or_text)


## Hint line of the card, as an i18n key or an already translated line.
func set_hint(text_key_or_text: String) -> void:
	if _finished:
		return
	if _hint != null:
		_hint.text = _tr(text_key_or_text)


## Shows `text_key_or_text` (an i18n key or an already translated line) at `progress` (0..1).
## Called from Rust once per stage and once per initial tile.
func set_stage(text_key_or_text: String, progress: float) -> void:
	if _finished:
		return
	var value := text_key_or_text
	if value.begins_with("loading.") or value.begins_with("intro.") or value.begins_with("home."):
		value = _tr(value)
	if _stage != null:
		_stage.text = value
	if _progress != null:
		_progress.value = clampf(progress, 0.0, 1.0)
	visible = true


## Hides the screen and stops the script. Idempotent: calling it twice is harmless. The layer
## ignores the mouse from the start, so nothing blocks the game after this.
func finish() -> void:
	_finished = true
	visible = false
	set_process(false)
	if _screen != null:
		_screen.mouse_filter = Control.MOUSE_FILTER_IGNORE


func _tr(key: String) -> String:
	var translated := tr(key)
	if translated != key:
		return translated
	return String(FALLBACK.get(key, key))


func _apply_theme() -> void:
	var style := get_node_or_null("/root/Style")
	var panel := DEFAULT_PANEL
	var ink := DEFAULT_INK
	var accent := DEFAULT_ACCENT
	var border_color := DEFAULT_BORDER
	var radius := 18
	var border := 2
	if style != null:
		panel = style.color("ui", "panel", panel)
		ink = style.color("ui", "ink", ink)
		accent = style.color("ui", "accent", accent)
		border_color = style.color("ui", "border", border_color)
		radius = int(style.number("ui", "corner_radius", float(radius)))
		border = int(style.number("ui", "border_width", float(border)))
	radius = clampi(radius, 0, 64)
	border = clampi(border, 0, 16)

	if _card != null:
		_card.add_theme_stylebox_override("panel", _panel_box(panel, border_color, border, radius))
	if _progress != null:
		var bar_radius := mini(radius, 6)
		_progress.add_theme_stylebox_override("background", _panel_box(panel.darkened(0.12), border_color, border, bar_radius))
		_progress.add_theme_stylebox_override("fill", _panel_box(accent, accent, 0, bar_radius))
	if _screen != null:
		var backdrop := _screen.get_node_or_null("Backdrop") as ColorRect
		if backdrop != null:
			backdrop.color = Color(ink.r, ink.g, ink.b, 0.55)
	for label in [_title, _stage, _hint]:
		if label != null:
			label.add_theme_color_override("font_color", ink)
	if _hint != null:
		_hint.add_theme_color_override("font_color", Color(ink.r, ink.g, ink.b, 0.8))


func _panel_box(bg: Color, border_color: Color, border: int, radius: int) -> StyleBoxFlat:
	var box := StyleBoxFlat.new()
	box.bg_color = bg
	box.border_color = border_color
	box.set_border_width_all(border)
	box.set_corner_radius_all(radius)
	return box
