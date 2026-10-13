#!/usr/bin/env python3
"""Convert a Curust slice job (CURUST_DUMP_JOB) into CuraEngine command line input: a resolved
settings JSON (-r) and binary STL files with the exact float32 vertices.

usage: job_to_cli.py JOB OUTDIR   -> prints the engine arguments (after `slice`)
"""
import json, os, struct, sys

def main():
    data = open(sys.argv[1], "rb").read()
    out = os.path.abspath(sys.argv[2])
    os.makedirs(out, exist_ok=True)
    pos = 8
    def u32():
        nonlocal pos
        v = struct.unpack_from("<I", data, pos)[0]; pos += 4; return v
    def i32():
        nonlocal pos
        v = struct.unpack_from("<i", data, pos)[0]; pos += 4; return v
    def bts():
        nonlocal pos
        n = u32(); b = data[pos:pos + n]; pos += n; return b
    def sets():
        return [(bts().decode(), bts().decode()) for _ in range(u32())]
    def f32s():
        nonlocal pos
        n = u32(); v = struct.unpack_from("<%df" % n, data, pos); pos += 4 * n; return v
    resolved = {"global": dict(sets())}
    for _ in range(u32()):
        e = i32()
        resolved["extruder.%d" % e] = dict(sets())
    lte = {}
    for _ in range(u32()):
        k = bts().decode(); lte[k] = "extruder.%d" % i32()
    resolved["limit_to_extruder"] = lte
    groups = u32()
    if groups != 1:
        print("only single mesh-group jobs are supported", file=sys.stderr)
    n = 0
    for _ in range(groups):
        sets()
        for _ in range(u32()):
            name = bts().decode(); v = f32s(); f32s(); bts(); st = dict(sets())
            path = os.path.join(out, "m%d.stl" % n); n += 1
            with open(path, "wb") as f:
                f.write(b"\0" * 80)
                f.write(struct.pack("<I", len(v) // 9))
                for i in range(0, len(v), 9):
                    f.write(struct.pack("<3f", 0, 0, 0))
                    f.write(struct.pack("<9f", *v[i:i + 9]))
                    f.write(b"\0\0")
            resolved[path] = st
    jp = os.path.join(out, "resolved.json")
    json.dump(resolved, open(jp, "w"))
    print("-r", jp)

main()
