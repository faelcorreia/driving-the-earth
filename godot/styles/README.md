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
| `palette.grass` | `#RRGGBB` | `#79A65A` | Terrain color where there is no ground cover. |
| `palette.cement` | `#RRGGBB` | `#B9B4AA` | Ground class `concrete`. |
| `palette.sidewalk` | `#RRGGBB` | `#CFC9BC` | Ground class `sidewalk`. |
| `palette.park` | `#RRGGBB` | `#5F8C4B` | Ground class `park`. |
| `palette.water` | `#RRGGBB` | `#5C93A6` | Ground class `water`. |
| `palette.road` | `#RRGGBB` | `#3B3F46` | Asphalt of the road ribbons and of the distant roads. |
| `palette.buildings` | array of 4 `#RRGGBB` | cream, brick, blue, sand | Facade and roof colors, picked per building. |
| `palette.lamp` | `#RRGGBB` | `#FFC24A` | Street lamp head and glow. |
| `palette.car` | `#RRGGBB` | `#D9483B` | Car body. |

### `light`

Colors used by the sky and by the sun/moon table. They are anchors, not a full lighting setup.

| Field | Type | Default | Effect |
| --- | --- | --- | --- |
| `sun_high` | `#RRGGBB` | `#FFF3DA` | Sun color near the zenith (midday). |
| `sun_low` | `#RRGGBB` | `#FFB27A` | Sun color near the horizon (sunrise and sunset). |
| `sky_day` | `#RRGGBB` | `#78B7E8` | Day sky above the horizon. |
| `sky_horizon` | `#RRGGBB` | `#E8F1F5` | Sky color at the horizon. |
| `shadow_tint` | `#RRGGBB` | `#8FA6C8` | Color mixed into the shadows. |

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
