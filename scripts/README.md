# Scripts

Standalone tools run from the repository root. Most are Python 3 and use only the standard
library, so no virtualenv or `pip install` is needed. The exception is `car_glb_lowpoly.py`, which
runs inside Blender (see below).

## `check_road_float.py`

Checks that no road centerline floats above the terrain the game draws.

```sh
python3 scripts/check_road_float.py casa-10km
```

It reads `cache/<map_id>/tiles/*.json` (the tiles are not versioned; bake them first with
`cargo run --release -p tilebake -- at <lat> <lon> <map_id>`), samples each tile `height` grid
on the same triangles as `tileformat::terrain::sample_local` (each cell split along the NE-SW
diagonal) and compares the result with each road point's `z`. The cap is
`profile::CAP_ABOVE_M` (0.5 m) plus a 1 cm tolerance for the `f32` heights and the JSON round
trip. `fine_height` is that same surface subdivided, so the coarse grid gives the same answer.

It prints the worst 10 pieces (tile, highway, name, worst point, coordinates, `z`, terrain
and deviation) and the totals. Exit codes: `0` no point above the cap, `1` at least one piece
with a point above 0.5 m + 1 cm, `2` the cache could not be read. `--cache DIR` points to
another cache root.

## `check_version.py`

Mechanical check of `[workspace.package] version` against the base branch. It fails when the
version is malformed, when a PR changes code without a bump, or when the bump is not exact
(patch +1, minor +1 with patch reset to 0, and so on).

```sh
python3 scripts/check_version.py --base origin/main
```

The rules are described in `CONTRIBUTING.md`. The `version-check` workflow runs it on every PR
to `main`. `scripts/test_check_version.py` covers it; run it from the `scripts/` folder with
`python3 -m unittest test_check_version`.

## `build_worldmap.py`

Builds the world map asset used by the home picker (`godot/assets/worldmap/world.bin`) from
pinned Natural Earth sources, checking each file against its sha256.

```sh
python3 scripts/build_worldmap.py
```

Re-running with the same sources gives the same bytes. The binary layout is documented at the
top of the script.

## `car_glb_lowpoly.py`

Turns a textured car GLB (the Tripo TR4 export) into the low-poly, texture-free car the game uses:
one body and four wheels, each wheel centered on its own axle. It runs inside Blender 4.5, not
with `python3`.

```sh
/path/to/blender -b -P scripts/car_glb_lowpoly.py -- \
  --input tripo_convert_<id>.glb --output godot/assets/car/car_tr4.glb \
  --ratio 0.2 --length 3.7 --track 1.4 --wheelbase 2.36 --wheel-radius 0.29 --preview <dir>
```

`--ratio` is the fraction of triangles kept per part, `--length` the car length in meters. The
optional `--track`, `--wheelbase` and `--wheel-radius` (meters) match the wheel positions and the
tire radius to the game's car node. `--preview DIR` writes two renders. The script prints the wheel
fits, the triangle counts and the body width: check them when the source changes. The output is
not versioned by the script; the game loads it from `godot/assets/car/car_tr4.glb` and falls back
to the procedural car if it is missing.
