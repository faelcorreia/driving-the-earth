# Driving the Earth

Drive a realistic small car (a light hatchback) through real places, built from open map data.
Rust (Godot 4 GDExtension) runtime plus an offline `tilebake` pipeline that turns OpenStreetMap
and elevation data into 250 m tiles.

Strongly inspired by [BeamNG.Earth](https://github.com/tecnagamer/beamng.earth) (real-world maps
streamed into BeamNG.drive). This project is independent, not affiliated with or endorsed by BeamNG
GmbH or the BeamNG.Earth authors; all credit for the idea goes to them.

Status: early work in progress. Priorities: 1) performance, 2) realistic driving feel.

## Download and play

For Windows (64-bit), no developer tools needed:

1. Open the [latest release](https://github.com/faelcorreia/driving-the-earth/releases/latest) and
   download `driving-the-earth-<version>-windows.zip`.
2. Extract the whole folder. The game needs the `.dll` next to `driving-the-earth.exe`, so do not
   move the `.exe` on its own.
3. Run `driving-the-earth.exe`. The release is not code-signed, so Windows SmartScreen shows
   "Windows protected your PC". Click **More info**, then **Run anyway**.
4. The first time, pick a place on the world map. The game then builds a small area (about 2 km)
   around it. This needs an internet connection: map data comes from OpenStreetMap (Overpass API)
   and elevation from Copernicus GLO-30. The built area is stored in the game's user data folder on
   your computer.

What to expect:

- The download is small: the zip holds only the game and its extension library. Map data is not
  included in it.
- The first build of an area takes noticeably longer than starting the game. The time depends on
  your connection and computer; no release timing has been measured yet.

Map data and attribution:

- Map data © OpenStreetMap contributors, ODbL: https://www.openstreetmap.org/copyright
- Elevation: Copernicus GLO-30 DEM (© DLR/ESA, Copernicus license).

## Layout
- `crates/tileformat`: tile data contract and units (everything in meters).
- `crates/tilebake`: offline pipeline (OSM via Overpass, Copernicus GLO-30 DEM, regional terrain packages).
- `crates/gdext`: Godot runtime (geometry, world, environment).
- `godot/`: Godot project (scenes, shaders, scripts).

## Running
1. Pick your home on the world map: press M, click a place, Confirm. It is saved to the game's
   preferences (`user://home.json`), the only source of the home. Without a saved home, or with one
   outside the loaded map, the world map opens at start.
2. Bake tiles into `cache/` (not versioned) with `tilebake` (`tilebake at <lat> <lon>`, see `crates/tilebake/src/main.rs`).
3. Build the extension (`cargo build`) and open `godot/` in Godot 4.

## Release builds
Releases are built by `.github/workflows/build.yml` for Linux (x86_64), Windows (x86_64) and macOS
(universal). Godot version: 4.7.2 (stable), the same one in `godot/project.godot`.

- A release package is only the executable plus the GDExtension library. **No baked data is included**:
  the map tiles stay local in `cache/`, produced by `tilebake` on your machine. Nothing in `cache/` or
  `*.local.json` is exported.
- The cache is `user://cache` (`EarthWorld.cache_root`), the user's data folder, in every build. In
  development, `res://../cache` (the repo's `cache/`) is used instead when it exists.
- The map opened is `EarthWorld.map_id` when set (the bot and the capture set it), else the last map
  opened (`user://last_map.txt`), else `casa-10km` in development only. With no map, the game emits
  `no_map`, shows the world map with a message and does not fail.
- Push a tag `vX.Y.Z` that matches `[workspace.package] version` in `Cargo.toml`. The workflow fails on
  a mismatch and then publishes a GitHub Release with the three zips
  (`driving-the-earth-<version>-<os>.zip`). A manual run (`workflow_dispatch`) builds the zips as
  workflow artifacts without publishing a release.
- macOS builds are not signed with an Apple Developer ID. Gatekeeper will show a "cannot be opened
  because the developer cannot be verified" alert. Open the app with right-click > Open, or run
  `xattr -dr com.apple.quarantine "Driving the Earth.app"`.

Local export (Godot 4.7.2 with export templates installed; create the output folder first, because
Godot does not create it):

```sh
godot --headless --path godot --import
mkdir -p dist/linux && godot --headless --path godot --export-release "Linux"
```

Presets are `Linux`, `Windows` and `macOS` in `godot/export_presets.cfg`. Output goes to `dist/`
(not versioned).

## Contributing
The whole repository is in English (code, identifiers, comments, logs, commit messages, docs). This
keeps the project easy to read and to extend with LLM-assisted code generation, which works best
with a single language.

Internationalization (i18n) is a core part of the project: every text shown in the interface must go
through i18n keys (currently en and pt-BR), never hardcoded strings.

Versioning rules (semver, major capped at 1, bumps checked in CI) are in [CONTRIBUTING.md](CONTRIBUTING.md).
Commit messages must not contain AI tool names or `Co-Authored-By` trailers; the only allowed
trailer line is `AI Generated` when AI wrote the change. Never commit personal data (addresses,
exact home coordinates, `home.local.json`, `cache/`).

## Data and attribution
This is a free, open-source (MIT), non-profit project. It aims to be a well-functioning terrain and
driving simulation engine grounded in Brazilian reality.

- Map data © OpenStreetMap contributors, ODbL: https://www.openstreetmap.org/copyright
- Elevation: Copernicus GLO-30 DEM (© DLR/ESA, Copernicus license).
- Terrain for Belo Horizonte derives from contour lines (1 m and 5 m) published by Prefeitura de
  Belo Horizonte / Prodabel (IDE-BHGeo, Decreto 17.209/2019), modified: resampled to a 10 m height
  grid. The source data is not redistributed in this repository. Check each source's license before
  redistributing baked data.

## Disclaimer
The data and simulation in this project must not be used for real-world decisions (navigation,
engineering, safety, legal or any other). Even though it tries to be as close to reality as possible,
it is subject to errors. Contributions that reduce those errors are always welcome.

## License
MIT (code). See `LICENSE`. Data sources keep their own licenses.
