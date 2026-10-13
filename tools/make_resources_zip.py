#!/usr/bin/env python3
"""Package Cura's resource files for Curust.

Curust reads its printer definitions, materials and profiles from UltiMaker Cura's resource
directory. On a computer without Cura, put the archive this script makes next to curust.exe
(or curust) as `curust-resources.zip`; Curust extracts it on first start.

usage: make_resources_zip.py CURA_RESOURCES_DIR [OUTPUT.zip]

CURA_RESOURCES_DIR is e.g. "C:\\Program Files\\UltiMaker Cura 5.12.0\\share\\cura\\resources"
(an installation folder that contains share/cura/resources works too).
The resource files are Cura's (LGPL-3.0 / CC0 as stated in Cura's repositories).
"""
import os, sys, zipfile

SUBDIRS = ["definitions", "extruders", "variants", "materials", "quality", "intent", "setting_visibility"]


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)
    src = os.path.abspath(sys.argv[1])
    for cand in (src, os.path.join(src, "share", "cura", "resources")):
        if os.path.isfile(os.path.join(cand, "definitions", "fdmprinter.def.json")):
            src = cand
            break
    else:
        sys.exit(f"{src} is not a Cura resources directory (no definitions/fdmprinter.def.json)")
    out = sys.argv[2] if len(sys.argv) > 2 else "curust-resources.zip"
    count = 0
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for sub in SUBDIRS:
            base = os.path.join(src, sub)
            if not os.path.isdir(base):
                continue
            for root, dirs, files in os.walk(base):
                dirs.sort()
                for f in sorted(files):
                    full = os.path.join(root, f)
                    rel = os.path.relpath(full, src).replace(os.sep, "/")
                    z.write(full, "resources/" + rel)
                    count += 1
    print(f"{out}: {count} files, {os.path.getsize(out) / 1e6:.1f} MB")


if __name__ == "__main__":
    main()
