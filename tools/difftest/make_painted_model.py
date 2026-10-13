#!/usr/bin/env python3
"""Create a painted test model the way Cura hands one to CuraEngine: a binary STL, a `.uv` file
(uint32 vertex count + one float (u, v) pair per face vertex, in face order) and a `.png` texture
whose `Description` text chunk holds the paint feature manifest (JSON: feature -> [first bit,
last bit] of the pixel value, the pixel value being its bytes read big-endian).

usage: make_painted_model.py <in.stl> <out-base> [--proj cyl|planar|sphere] [--size WxH]
           [--color rgb|rgba|gray|ga] [--gamma N] [--feature name=lo,hi ...] --paint "<python expr>"

The paint expression is evaluated per texel with `u`, `v` (texel centre, 0..1) and must return a
dict feature -> value, e.g. --feature extruder=0,1 --paint "{'extruder': 1 if u < 0.3 else 0}".
"""
import argparse, json, math, struct, zlib


def read_stl(path):
    data = open(path, "rb").read()
    tris = []
    if data[:5].lower() == b"solid" and b"facet" in data[:400]:
        verts = []
        for line in data.decode(errors="replace").splitlines():
            p = line.split()
            if p and p[0] == "vertex":
                verts.append(tuple(float(c) for c in p[1:4]))
                if len(verts) == 3:
                    tris.append(verts)
                    verts = []
        return tris
    (n,) = struct.unpack_from("<I", data, 80)
    for i in range(n):
        f = struct.unpack_from("<12f", data, 84 + 50 * i)
        tris.append([f[3:6], f[6:9], f[9:12]])
    return tris


def write_stl(path, tris):
    out = bytearray(b"painted test model".ljust(80, b" "))
    out += struct.pack("<I", len(tris))
    for t in tris:
        out += struct.pack("<3f", 0, 0, 0)
        for v in t:
            out += struct.pack("<3f", *v)
        out += b"\0\0"
    open(path, "wb").write(out)


def png_chunk(kind, payload):
    c = kind + payload
    return struct.pack(">I", len(payload)) + c + struct.pack(">I", zlib.crc32(c) & 0xFFFFFFFF)


def write_png(path, w, h, channels, pixel_bytes, description, gamma=None):
    color_type = {1: 0, 2: 4, 3: 2, 4: 6}[channels]
    raw = bytearray()
    for y in range(h):
        raw.append(0)
        raw += pixel_bytes[y * w * channels:(y + 1) * w * channels]
    out = b"\x89PNG\r\n\x1a\n"
    out += png_chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, color_type, 0, 0, 0))
    if gamma is not None:
        out += png_chunk(b"gAMA", struct.pack(">I", gamma))
    out += png_chunk(b"tEXt", b"Description\0" + description.encode())
    out += png_chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    out += png_chunk(b"IEND", b"")
    open(path, "wb").write(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("stl")
    ap.add_argument("out")
    ap.add_argument("--proj", default="cyl")
    ap.add_argument("--size", default="64x64")
    ap.add_argument("--color", default="rgb")
    ap.add_argument("--feature", action="append", default=[])
    ap.add_argument("--paint", required=True)
    ap.add_argument("--manifest", default=None, help="JSON manifest to store instead of the --feature list")
    ap.add_argument("--gamma", type=int, default=None, help="write a gAMA chunk (e.g. 100000 for linear)")
    o = ap.parse_args()

    tris = read_stl(o.stl)
    xs = [v[0] for t in tris for v in t]
    ys = [v[1] for t in tris for v in t]
    zs = [v[2] for t in tris for v in t]
    cx, cy, cz = (min(xs) + max(xs)) / 2, (min(ys) + max(ys)) / 2, (min(zs) + max(zs)) / 2
    sx, sz = max(xs) - min(xs) or 1, max(zs) - min(zs) or 1

    def uv(p):
        x, y, z = p
        if o.proj == "planar":  # front view: u along x, v along z
            return (x - min(xs)) / sx, (z - min(zs)) / sz
        if o.proj == "sphere":
            r = math.sqrt((x - cx) ** 2 + (y - cy) ** 2 + (z - cz) ** 2) or 1
            return math.atan2(y - cy, x - cx) / (2 * math.pi) + 0.5, math.acos(max(-1, min(1, (z - cz) / r))) / math.pi
        return math.atan2(y - cy, x - cx) / (2 * math.pi) + 0.5, (z - min(zs)) / sz

    uvs = [uv(v) for t in tris for v in t]
    w, h = (int(c) for c in o.size.split("x"))
    channels = {"gray": 1, "ga": 2, "rgb": 3, "rgba": 4}[o.color]
    features = {}
    for f in o.feature:
        name, rng = f.split("=")
        lo, hi = (int(c) for c in rng.split(","))
        features[name] = [lo, hi]
    pixels = bytearray()
    for y in range(h):
        for x in range(w):
            values = eval(o.paint, {"math": math}, {"u": (x + 0.5) / w, "v": (y + 0.5) / h})
            value = 0
            for name, val in values.items():
                lo, hi = features[name]
                value |= (int(val) & ((1 << (hi - lo + 1)) - 1)) << lo
            pixels += value.to_bytes(channels, "big")
    write_stl(o.out + ".stl", tris)
    with open(o.out + ".uv", "wb") as f:
        f.write(struct.pack("<I", len(uvs)))
        for u, v in uvs:
            f.write(struct.pack("<2f", u, v))
    write_png(o.out + ".png", w, h, channels, pixels, o.manifest or json.dumps(features), o.gamma)


if __name__ == "__main__":
    main()
