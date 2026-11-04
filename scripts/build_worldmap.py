#!/usr/bin/env python3
"""Builds the world map asset used by the home picker (F17b).

Source: Natural Earth 1:50m vector data (land, borders) and 1:10m populated places (public domain, https://www.naturalearthdata.com),
read from the GeoJSON mirror in nvkelso/natural-earth-vector at a pinned commit. Each file is
checked against its sha256 before use. Output: godot/assets/worldmap/world.bin (little endian).

Usage: python3 scripts/build_worldmap.py [--cache DIR] [--out FILE]
Stdlib only. Re-running with the same pinned sources gives the same bytes.

Binary layout (version 1):
  magic b"DTEW", u16 version, u16 reserved
  u32 ring_count, then per ring: u8 hole (0 land, 1 water inside land), u32 n, n * (i32 lon, i32 lat)
  u32 line_count, then per line: u32 n, n * (i32 lon, i32 lat)          (country borders)
  u32 city_count, then per city: u8 tier (0 largest), i32 lon, i32 lat, u8 len, len bytes UTF-8 name
Coordinates are degrees times 100000 (about 1 m). Latitude is clamped to +-85 degrees.
"""
import argparse
import hashlib
import json
import math
import os
import struct
import sys
import urllib.request

NE_COMMIT = "ca96624a56bd078437bca8184e78163e5039ad19"
BASE_URL = f"https://raw.githubusercontent.com/nvkelso/natural-earth-vector/{NE_COMMIT}/geojson/"
SOURCES = {
    "ne_50m_land.geojson": "e874b27a51d146452be360cafb3cc50c86001074a67d534113e6534682f9826b",
    "ne_50m_admin_0_boundary_lines_land.geojson": "2faac4f6b34386f3d21b6e018cf151f241f00e5c936d44dd17d7d9bfb147fa48",
    "ne_10m_populated_places_simple.geojson": "fd3fa867a320cbd5c5b6bb5bc550afeec2939fb2cef688e508007282a55ac42f",
}
LAT_LIMIT = 85.0
SIMPLIFY_DEG = 0.02          # Douglas-Peucker tolerance, about 2 km
MIN_RING_DEG = 0.2           # smallest land ring kept, in degrees
SCALE = 100000               # 1e-5 degree per unit
TIER_POP = [(5_000_000, 0), (1_000_000, 1), (200_000, 2)]  # else tier 3
NAME_MAX = 255


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def fetch(name, cache):
    """Returns the local path of a source file, downloading it if missing or corrupt."""
    os.makedirs(cache, exist_ok=True)
    path = os.path.join(cache, name)
    if os.path.exists(path) and sha256(path) == SOURCES[name]:
        return path
    tmp = path + ".part"
    urllib.request.urlretrieve(BASE_URL + name, tmp)
    if sha256(tmp) != SOURCES[name]:
        os.remove(tmp)
        sys.exit(f"sha256 mismatch for {name}; the pinned commit or hash is wrong")
    os.replace(tmp, path)
    return path


def unwrap(coords):
    """Lon/lat pairs to continuous (lon, lat), lat clamped; crossing 180 keeps the line whole."""
    out, prev_raw, offset = [], None, 0.0
    for lon, lat in coords:
        if prev_raw is not None:
            if lon - prev_raw > 180:
                offset -= 360
            elif lon - prev_raw < -180:
                offset += 360
        prev_raw = lon
        out.append((lon + offset, min(max(lat, -LAT_LIMIT), LAT_LIMIT)))
    dedup = [out[0]]
    for p in out[1:]:
        if p != dedup[-1]:
            dedup.append(p)
    return dedup


def simplify(pts, tol):
    """Douglas-Peucker; keeps the first and last point."""
    if len(pts) < 3:
        return pts
    keep = [False] * len(pts)
    keep[0] = keep[-1] = True
    stack = [(0, len(pts) - 1)]
    while stack:
        a, b = stack.pop()
        if b <= a + 1:
            continue
        ax, ay = pts[a]
        bx, by = pts[b]
        dx, dy = bx - ax, by - ay
        len2 = dx * dx + dy * dy
        best, best_d = -1, -1.0
        for i in range(a + 1, b):
            px, py = pts[i]
            if len2 == 0:
                d = math.hypot(px - ax, py - ay)
            else:
                t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / len2))
                d = math.hypot(px - (ax + t * dx), py - (ay + t * dy))
            if d > best_d:
                best, best_d = i, d
        if best_d > tol:
            keep[best] = True
            stack.append((a, best))
            stack.append((best, b))
    return [p for p, k in zip(pts, keep) if k]


def quantize(pts):
    return [(round(x * SCALE), round(y * SCALE)) for x, y in pts]


def rings_of(geom):
    """Polygon or MultiPolygon to (hole flag, coordinate list) per ring."""
    if geom["type"] == "Polygon":
        polys = [geom["coordinates"]]
    elif geom["type"] == "MultiPolygon":
        polys = geom["coordinates"]
    else:
        return []
    out = []
    for poly in polys:
        for i, ring in enumerate(poly):
            out.append((1 if i > 0 else 0, ring))
    return out


def lines_of(geom):
    if geom["type"] == "LineString":
        return [geom["coordinates"]]
    if geom["type"] == "MultiLineString":
        return geom["coordinates"]
    return []


def tier_of(pop):
    for limit, tier in TIER_POP:
        if pop >= limit:
            return tier
    return 3


def build(cache):
    paths = {name: fetch(name, cache) for name in SOURCES}

    def load(name):
        with open(paths[name], encoding="utf-8") as f:
            return json.load(f)["features"]

    rings, lines, cities = [], [], []
    for feat in load("ne_50m_land.geojson"):
        for hole, ring in rings_of(feat["geometry"]):
            pts = simplify(unwrap(ring), SIMPLIFY_DEG)
            q = quantize(pts)
            xs = [p[0] for p in pts]
            ys = [p[1] for p in pts]
            # Islets under 0.2 degrees are invisible at world scale, and a few tiny Pacific ones
            # are self-intersecting in the source, which the polygon fill cannot draw.
            if len(q) >= 4 and max(max(xs) - min(xs), max(ys) - min(ys)) >= MIN_RING_DEG:
                rings.append((hole, q))
    for feat in load("ne_50m_admin_0_boundary_lines_land.geojson"):
        for line in lines_of(feat["geometry"]):
            q = quantize(simplify(unwrap(line), SIMPLIFY_DEG))
            if len(q) >= 2:
                lines.append(q)
    for feat in load("ne_10m_populated_places_simple.geojson"):
        props = feat["properties"]
        name = props.get("name")
        pop = props.get("pop_max") or 0
        lon, lat = feat["geometry"]["coordinates"]
        if not name:
            continue
        name_b = name.encode("utf-8")[:NAME_MAX]
        cities.append((-pop, tier_of(pop), round(lon * SCALE),
                       round(min(max(lat, -LAT_LIMIT), LAT_LIMIT) * SCALE), name_b))
    cities.sort(key=lambda c: (c[0], c[4]))  # largest first, then name: deterministic

    out = bytearray(b"DTEW" + struct.pack("<HH", 1, 0))
    out += struct.pack("<I", len(rings))
    for hole, q in rings:
        out += struct.pack("<BI", hole, len(q))
        for x, y in q:
            out += struct.pack("<ii", x, y)
    out += struct.pack("<I", len(lines))
    for q in lines:
        out += struct.pack("<I", len(q))
        for x, y in q:
            out += struct.pack("<ii", x, y)
    out += struct.pack("<I", len(cities))
    for _, tier, lon, lat, name_b in cities:
        out += struct.pack("<BiiB", tier, lon, lat, len(name_b)) + name_b
    return bytes(out), len(rings), sum(len(q) for _, q in rings), len(lines), len(cities)


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    root = os.path.dirname(here)
    ap = argparse.ArgumentParser()
    ap.add_argument("--cache", default=os.path.join(root, "target", "worldmap-src"))
    ap.add_argument("--out", default=os.path.join(root, "godot", "assets", "worldmap", "world.bin"))
    args = ap.parse_args()
    data, n_rings, n_pts, n_lines, n_cities = build(args.cache)
    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    with open(args.out, "wb") as f:
        f.write(data)
    print(f"wrote {args.out}: {len(data)} bytes, rings {n_rings} ({n_pts} points), "
          f"lines {n_lines}, cities {n_cities}")


if __name__ == "__main__":
    main()
