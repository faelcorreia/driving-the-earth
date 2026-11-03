# Driving the Earth

Drive a realistic small car (a light hatchback) through real places, built from open map data.
Rust (Godot 4 GDExtension) runtime plus an offline `tilebake` pipeline that turns OpenStreetMap
and elevation data into 250 m tiles.

Strongly inspired by [BeamNG.Earth](https://github.com/tecnagamer/beamng.earth) (real-world maps
streamed into BeamNG.drive). This project is independent, not affiliated with or endorsed by BeamNG
GmbH or the BeamNG.Earth authors; all credit for the idea goes to them.

Status: early work in progress. Priorities: 1) performance, 2) realistic driving feel.

## Layout
- `crates/tileformat`: tile data contract and units (everything in meters).
- `crates/tilebake`: offline pipeline (OSM via Overpass, Copernicus GLO-30 DEM, regional terrain packages).
- `crates/gdext`: Godot runtime (geometry, world, environment).
- `godot/`: Godot project (scenes, shaders, scripts).

## Running
1. Pick a start point: set `DTE_HOME="lat,lon"` or create an untracked `home.local.json`
   (`{"lat": -19.9191, "lon": -43.9386, "street": "optional"}`). Default: Praça Sete, Belo Horizonte.
2. Bake tiles into `cache/` (not versioned) with `tilebake` (see `crates/tilebake/src/main.rs`).
3. Build the extension (`cargo build`) and open `godot/` in Godot 4.

## Release builds
Releases are built by `.github/workflows/build.yml` for Linux (x86_64), Windows (x86_64) and macOS
(universal). Godot version: 4.7.2 (stable), the same one in `godot/project.godot`.

- A release package is only the executable plus the GDExtension library. **No baked data is included**:
  the map tiles stay local in `cache/`, produced by `tilebake` on your machine. Nothing in `cache/` or
  `*.local.json` is exported.
- The cache path is still `res://../cache` (`EarthWorld.cache_root`). In an exported build it resolves
  relative to the directory the game is launched from, not to the executable: from `dist/linux` the
  game looks for `dist/cache`. Where the release should look for `cache/` is an open decision.
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
