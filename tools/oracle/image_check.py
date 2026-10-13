#!/usr/bin/env python3
"""Compare Curust's image import (Qt-equivalent decoding and scaling, ImageReader meshes) with the
reference that image_ref.py produces with Cura's own Qt and ImageReader (run it with Cura's Python).
usage: image_check.py [FILE] (in a folder with corpus/ and ref/)"""
import json, os, subprocess, sys, struct
import os as _os, tempfile as _tempfile
# Paths (see tools/README.md): CURUST_WORK is a scratch folder for builds and results, CURA_ROOT an
# extracted UltiMaker Cura 5.12 AppImage.
WORK = _os.environ.get("CURUST_WORK", _os.path.join(_tempfile.gettempdir(), "curust-work"))
CURA_ROOT = _os.environ.get("CURA_ROOT", WORK + "/cura")
REPO = _os.path.abspath(_os.path.join(_os.path.dirname(_os.path.abspath(__file__)), "..", ".."))
CLI = REPO + "/target/release/curust-cli"
specs = ["size=120,peak=2.5,base=0.4,smoothing=1,lighter_is_higher=0,transparency=1,transmittance=50",
         "size=80,peak=3.0,base=1.0,smoothing=3,lighter_is_higher=1,transparency=0,transmittance=50"]
only = sys.argv[1] if len(sys.argv) > 1 else None
ok_all = 0; n = 0
for name in sorted(os.listdir("corpus")):
    if only and name != only: continue
    n += 1
    ref = os.path.join("ref", name); mine = os.path.join("mine", name)
    subprocess.run([CLI, "image-debug", os.path.join("corpus", name), mine] + specs, capture_output=True)
    ri = json.load(open(os.path.join(ref, "info.json")))
    try: mi = json.load(open(os.path.join(mine, "info.json")))
    except Exception as e: print(name, "NO OUTPUT"); continue
    probs = []
    for k in ("null", "format", "width", "height", "alpha"):
        if ri.get(k) != mi.get(k): probs.append(f"{k}: qt={ri.get(k)} curust={mi.get(k)}")
    for f in sorted(os.listdir(ref)):
        if f == "info.json": continue
        a = open(os.path.join(ref, f), "rb").read()
        p = os.path.join(mine, f)
        if not os.path.exists(p): probs.append(f"{f} missing"); continue
        b = open(p, "rb").read()
        if a != b:
            if f.endswith(".u32") and len(a) == len(b):
                ua = struct.unpack("<%dI" % (len(a)//4), a); ub = struct.unpack("<%dI" % (len(b)//4), b)
                d = [(i, hex(x), hex(y)) for i, (x, y) in enumerate(zip(ua, ub)) if x != y]
                probs.append(f"{f}: {len(d)} px differ, first {d[:3]}")
            elif f.endswith(".f32") and len(a) == len(b):
                fa = struct.unpack("<%df" % (len(a)//4), a); fb = struct.unpack("<%df" % (len(b)//4), b)
                d = [(i, x, y) for i, (x, y) in enumerate(zip(fa, fb)) if x != y]
                probs.append(f"{f}: {len(d)} floats differ of {len(fa)}, first {d[:3]}")
            else:
                probs.append(f"{f}: size {len(a)} vs {len(b)}")
    if probs:
        print(name, "DIFF:", "; ".join(probs))
    else:
        ok_all += 1; print(name, "IDENTICAL")
print(f"{ok_all}/{n} identical")
