# Visual themes

A theme is a short JSON file with only the fields that change the look. There is no font,
spacing or layout field: those belong to the UI code, not to the theme.

`lowpoly.json` is the default. `realistic.json` is the same theme with smooth shading, a smaller
bevel and a more desaturated palette.

## Choosing a theme

- `--style <id>` after `--` on the command line, e.g. `godot --path godot -- --style realistic`.
- The `EarthWorld.style_id` export, when set (the command line wins).
- Without either, `lowpoly`.

## Fields

### `id`

Theme name. It should match the file name (`lowpoly.json` has `"id": "lowpoly"`).

### `world`

| Field | Type | Default | Effect |
| --- | --- | --- | --- |
| `textures` | bool | `false` | Reserved for textured worlds; every current theme keeps it `false`. |
| `flat_shading` | bool | `true` | `true`: faceted normals (low-poly); `false`: smooth normals where the geometry supports them. |
| `bevel_m` | number (m) | `0.22` | Size of the chamfer on vertical corners, in meters. Clamped to `0..1`. |
| `bevel_segments` | integer | `1` | Segments of each chamfer. Clamped to `1..3`. |
| `shadow_softness` | number | `1.0` | Shadow penumbra. Clamped to `0..2`. |
| `palette.grass` | `#RRGGBB` | `#8CA65A` | Terrain color where there is no ground cover (grama). |
| `palette.cement` | `#RRGGBB` | `#B7AB99` | Ground class `concrete` (pedra). |
| `palette.sidewalk` | `#RRGGBB` | `#F2E2C4` | Ground class `sidewalk` (creme). |
| `palette.park` | `#RRGGBB` | `#6E8F5C` | Ground class `park` (verde). |
| `palette.water` | `#RRGGBB` | `#3C6B73` | Ground class `water` (petróleo). |
| `palette.road` | `#RRGGBB` | `#4F4B57` | Asphalt of the road ribbons and of the distant roads. Blue-gray, never below `#4A4650`. |
| `palette.buildings` | array of 4 `#RRGGBB` | creme, terracota, ocre, pêssego | Wall and roof colors, picked per building. Warm tones only. |
| `palette.lamp` | `#RRGGBB` | `#E9B44C` | Street lamp head and glow. |
| `palette.car` | `#RRGGBB` | `#C56A4A` | Car body. |

The lowpoly palette has 12 color slots (the `palette` block), under the 14-slot limit of the
family. No theme should use pure `#000000` or `#FFFFFF`.

### `light`

Colors used by the sky, by the sun/moon table and by the shaded faces. They are anchors, not a
full lighting setup.

| Field | Type | Default | Effect |
| --- | --- | --- | --- |
| `sun_high` | `#RRGGBB` | `#FFF3DA` | Sun color near the zenith (midday). |
| `sun_low` | `#RRGGBB` | `#E8875A` | Sun color near the horizon (sunrise and sunset): coral. |
| `sky_day` | `#RRGGBB` | `#8EA3CF` | Day sky above the horizon: muted lilac-blue. |
| `sky_horizon` | `#RRGGBB` | `#F6D5C0` | Sky color at the horizon: pale peach. |
| `shadow_tint` | `#RRGGBB` | `#6B5A78` | Color mixed into the shadows (lilac, never gray or black). |
| `fill` | `#RRGGBB` or absent | absent (off) | Cold fill mixed into the shaded faces (`#7C8CD9` in lowpoly). Absent: no fill, the look of the older themes. |
| `atmosphere` | bool | `false` | `true`: warm haze in the `Environment` fog, soft glow and soft SSAO. `false`: off, as before. |

### `ui`

| Field | Type | Default | Effect |
| --- | --- | --- | --- |
| `panel` | `#RRGGBB` | `#F4EFE4` | Panel and button background. |
| `ink` | `#RRGGBB` | `#2E2A26` | Text. |
| `accent` | `#RRGGBB` | `#D9744A` | Highlight and focus color. |
| `border` | `#RRGGBB` | `#8C7F6B` | Border. |
| `border_width` | integer (px) | `2` | Border width. Clamped to `0..16`. |
| `corner_radius` | integer (px) | `18` | Corner radius. Clamped to `0..64`. |

Every field is optional: a missing field keeps the `lowpoly` default. A broken file or an
unknown color also falls back to the default, with one warning in the log.

## Creating a theme

1. Copy `lowpoly.json` to `<id>.json` (the id must match the file name).
2. Change `"id"` and the colors you want.
3. Keep the field names; do not add new ones.
4. Run with `-- --style <id>` and check the log line that reports the active style and palette.
