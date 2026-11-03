# Driving the Earth

Drive a realistic small car (a light hatchback) through real places, built from open map data.
Rust (Godot 4 GDExtension) runtime plus an offline `tilebake` pipeline that turns OpenStreetMap
and elevation data into 250 m tiles.

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

## Contributing
Code, identifiers, logs and commit messages are in English; UI text goes through i18n keys (en, pt-BR).
Commit messages must not contain AI tool names or `Co-Authored-By` trailers; the only allowed
trailer line is `AI Generated` when AI wrote the change. Never commit personal data (addresses,
exact home coordinates, `home.local.json`, `cache/`).

## Data and attribution
- Map data © OpenStreetMap contributors, ODbL: https://www.openstreetmap.org/copyright
- Elevation: Copernicus GLO-30 DEM (© DLR/ESA, Copernicus license).
- Regional terrain packages may derive from municipal open data (e.g. PBH); check each source's license before redistributing baked data.

## License
MIT (code). See `LICENSE`. Data sources keep their own licenses.
