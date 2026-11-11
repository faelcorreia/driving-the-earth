extends Node
## Active visual theme (item 118), as the `Style` autoload.
##
## Reads the same short JSON as the Rust reader (`res://styles/<id>.json`) for the Godot-only
## consumers (loading screen, UI theme, car mesh). World rendering reads the theme in Rust; this
## side never decides how the world is built.
##
## The active id comes from `--style <id>` in `OS.get_cmdline_user_args()`, else `lowpoly`.
## A missing or broken file warns once and keeps the built-in defaults (equal to `lowpoly.json`).

const DEFAULT_ID := "lowpoly"
const STYLE_DIR := "res://styles"

## Built-in defaults, equal to `styles/lowpoly.json`. Every field is optional in a theme file, so
## these values fill whatever a theme leaves out.
const DEFAULTS := {
	"id": "lowpoly",
	"world": {
		"textures": false,
		"flat_shading": true,
		"bevel_m": 0.22,
		"bevel_segments": 1,
		"shadow_softness": 1.0,
		"palette": {
			"grass": "#79A65A",
			"cement": "#B9B4AA",
			"sidewalk": "#CFC9BC",
			"park": "#5F8C4B",
			"water": "#5C93A6",
			"road": "#3B3F46",
			"buildings": ["#E4D5B7", "#D78E68", "#A9C3D0", "#C7A87D"],
			"lamp": "#FFC24A",
			"car": "#D9483B",
		},
	},
	"light": {
		"sun_high": "#FFF3DA",
		"sun_low": "#FFB27A",
		"sky_day": "#78B7E8",
		"sky_horizon": "#E8F1F5",
		"shadow_tint": "#8FA6C8",
	},
	"ui": {
		"panel": "#F4EFE4",
		"ink": "#2E2A26",
		"accent": "#D9744A",
		"border": "#8C7F6B",
		"border_width": 2,
		"corner_radius": 18,
	},
}

var _id := DEFAULT_ID
var _data: Dictionary = {}
var _loaded := false


func _ready() -> void:
	reload()


## Reads the theme again. Safe to call more than once (the defaults are restored first).
func reload() -> void:
	_id = _theme_id_from_args()
	_data = DEFAULTS.duplicate(true)
	_loaded = false
	var path := "%s/%s.json" % [STYLE_DIR, _id]
	if not FileAccess.file_exists(path):
		push_warning("style '%s' not found at %s: using the built-in defaults" % [_id, path])
		return
	var parsed: Variant = JSON.parse_string(FileAccess.get_file_as_string(path))
	if typeof(parsed) != TYPE_DICTIONARY:
		push_warning("style '%s' is not valid JSON: using the built-in defaults" % _id)
		return
	_merge(_data, parsed)
	var json_id: Variant = parsed.get("id")
	if json_id is String and not json_id.strip_edges().is_empty():
		_id = json_id.strip_edges()
	_loaded = true


## Id of the active theme.
func id() -> String:
	return _id


## True when the theme file was read; false when the built-in defaults are in use.
func has_style() -> bool:
	return _loaded


## Color of a dotted `section.key` (`color("world.palette", "grass")`, `color("ui", "panel")`),
## or `fallback`.
func color(section: String, key: String, fallback := Color.WHITE) -> Color:
	var value: Variant = _entry(section, key)
	if value is String:
		return Color.from_string(value, fallback)
	return fallback


## Number of a dotted `section.key` (`number("world", "bevel_m")`,
## `number("ui", "corner_radius")`), or `fallback`.
func number(section: String, key: String, fallback := 0.0) -> float:
	var value: Variant = _entry(section, key)
	if value is float or value is int:
		return float(value)
	return fallback


## Boolean of a dotted `section.key` (`flag("world", "flat_shading")`), or `fallback`.
func flag(section: String, key: String, fallback := false) -> bool:
	var value: Variant = _entry(section, key)
	if value is bool:
		return value
	return fallback


## Array of colors of a section key (`colors("world.palette", "buildings")`); empty when absent.
func colors(section: String, key: String) -> Array[Color]:
	var out: Array[Color] = []
	var value: Variant = _entry(section, key)
	if value is Array:
		for item in value:
			if item is String:
				out.append(Color.from_string(item, Color.WHITE))
	return out


func _entry(section: String, key: String) -> Variant:
	return _value("%s.%s" % [section, key])


## Walks a dotted path such as `world.palette.grass`.
func _value(path: String) -> Variant:
	var node: Variant = _data
	for part in path.split(".", false):
		if not node is Dictionary:
			return null
		node = node.get(part)
	return node


func _theme_id_from_args() -> String:
	var args := OS.get_cmdline_user_args()
	for i in args.size():
		var arg := String(args[i])
		if arg == "--style" and i + 1 < args.size():
			var value := String(args[i + 1]).strip_edges()
			if not value.is_empty():
				return value
		elif arg.begins_with("--style="):
			var value := arg.trim_prefix("--style=").strip_edges()
			if not value.is_empty():
				return value
	return DEFAULT_ID


## Deep merge of `over` into `base`, so a theme only states the fields that differ.
func _merge(base: Dictionary, over: Dictionary) -> void:
	for key in over:
		if base.get(key) is Dictionary and over[key] is Dictionary:
			_merge(base[key], over[key])
		else:
			base[key] = over[key]
