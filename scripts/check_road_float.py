#!/usr/bin/env python3
"""Checks that no road centerline floats above the terrain the game draws.

Usage: python3 scripts/check_road_float.py <map_id> [--cache DIR]

Reads `cache/<map_id>/tiles/*.json` (not versioned) and compares each road point's
`z` with the terrain under it. The terrain is sampled from the tile `height` grid on
the same triangles as `tileformat::terrain::sample_local` (each cell split along the
NE-SW diagonal, the surface the game draws), not bilinear: on a slope the two differ,
which is exactly how a road used to float unnoticed. `fine_height` is the same surface
subdivided, so sampling the coarse grid gives the same answer.

A point violates when it is more than `CAP_ABOVE_M` (0.5 m) plus `TOL_M` (0.01 m) above
the terrain, the cap of the bake (`profile::CAP_ABOVE_M`) with a 1 cm margin for the
float32 heights and the JSON round trip. A piece (one road polyline in one tile) violates
when any of its points does. Prints the worst 10 pieces and the totals.
Exit codes: 0 no violation, 1 violation, 2 could not read the cache.
"""

from __future__ import annotations

import argparse
import heapq
import json
import math
import sys
from pathlib import Path

CAP_ABOVE_M = 0.5
TOL_M = 0.01
SHOW = 10


def sample_local(height: dict, tile_m: float, lx: float, ly: float) -> float:
    """Terrain height at local `(lx, ly)`, on the NE-SW triangles of `height`."""
    n = int(height["cols"])
    values = height["values"]
    cell = tile_m / (n - 1)
    u = lx / cell
    v = ly / cell
    j0 = min(max(int(math.floor(u)), 0), n - 2)
    i0 = min(max(int(math.floor(v)), 0), n - 2)
    fu = u - j0
    fv = v - i0

    def at(i: int, j: int) -> float:
        return float(values[i * n + j])

    a = at(i0, j0)
    b = at(i0, j0 + 1)
    c = at(i0 + 1, j0)
    if fu + fv <= 1.0:
        return a + (b - a) * fu + (c - a) * fv
    d = at(i0 + 1, j0 + 1)
    return d + (c - d) * (1.0 - fu) + (b - d) * (1.0 - fv)


def check(map_id: str, cache: Path) -> int:
    meta_path = cache / map_id / "meta.json"
    tiles_dir = cache / map_id / "tiles"
    try:
        meta = json.loads(meta_path.read_text())
        tile_m = float(meta["tile_size_m"])
        files = sorted(tiles_dir.glob("*.json"))
    except (OSError, ValueError, KeyError) as exc:
        print(f"cannot read {meta_path}: {exc}", file=sys.stderr)
        return 2
    if not files:
        print(f"no tiles in {tiles_dir}", file=sys.stderr)
        return 2

    limit = CAP_ABOVE_M + TOL_M
    over_half = over_one = over_cap = pieces = points = 0
    worst: list[tuple[float, str]] = []
    for path in files:
        try:
            tile = json.loads(path.read_text())
        except (OSError, ValueError) as exc:
            print(f"cannot read {path}: {exc}", file=sys.stderr)
            return 2
        x, y = int(tile["x"]), int(tile["y"])
        height = tile["height"]
        for road in tile.get("roads", []):
            z = road.get("z") or []
            if not z or not road.get("points"):
                continue
            pieces += 1
            peak = (float("-inf"), 0, [0.0, 0.0], 0.0, 0.0)
            for k, (p, zi) in enumerate(zip(road["points"], z)):
                points += 1
                lx = p[0] - x * tile_m
                ly = p[1] - y * tile_m
                ground = sample_local(height, tile_m, lx, ly)
                dev = zi - ground
                if dev > peak[0]:
                    peak = (dev, k, p, zi, ground)
            dev, k, p, zi, ground = peak
            if dev > 0.5:
                over_half += 1
            if dev > 1.0:
                over_one += 1
            if dev > limit:
                over_cap += 1
            name = road.get("name") or "-"
            label = (
                f"tile ({x},{y}) {road.get('highway', '?')} \"{name}\" "
                f"point {k} ({p[0]:.2f},{p[1]:.2f}): z={zi:.2f} "
                f"ground={ground:.2f} dev=+{dev:.2f} m"
            )
            if len(worst) < SHOW:
                heapq.heappush(worst, (dev, label))
            elif dev > worst[0][0]:
                heapq.heapreplace(worst, (dev, label))

    print(f"{map_id}: {pieces} road pieces ({points} points) in {len(files)} tiles")
    if worst:
        print("worst pieces above the drawn terrain:")
        for dev, label in sorted(worst, reverse=True):
            print(f"  +{dev:.2f} m  {label}")
    print(
        f"pieces above 0.50 m: {over_half}   above 1.00 m: {over_one}   "
        f"above {limit:.2f} m: {over_cap}   "
        f"max: {max((d for d, _ in worst), default=0.0):.2f} m"
    )
    if over_cap:
        print(
            f"FAIL: {over_cap} road pieces with a point more than {limit:.2f} m "
            "above the drawn terrain"
        )
        return 1
    print(f"OK: no road point more than {limit:.2f} m above the drawn terrain")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("map_id", help="map id under cache/ (for example casa-10km)")
    parser.add_argument(
        "--cache",
        type=Path,
        default=Path(__file__).resolve().parent.parent / "cache",
        help="cache root (default: <repo>/cache)",
    )
    args = parser.parse_args()
    return check(args.map_id, args.cache)


if __name__ == "__main__":
    sys.exit(main())
