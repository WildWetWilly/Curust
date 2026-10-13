#!/usr/bin/env python3
"""Is the official CuraEngine deterministic? Run it repeatedly with an IDENTICAL command line.

For every job directory made by engine_check.py (job.bin + curust.gcode), the official
CuraEngine 5.12 binary slices the same input N times (same arguments, same output path, same
environment; only address-space layout randomisation differs between the runs), then N/2 times
with ASLR disabled (`setarch -R`). The G-code bodies are hashed (Griffin build/UUID lines
excluded). More than one hash for one job means Cura's own engine gives different G-code for the
same input from one run to the next.

usage: engine_rerun.py ENGINE_CHECK_DIR [N]
env:   OFFICIAL_ENGINE (default tools/oracle/engine_m1.sh)
"""
import hashlib, os, subprocess, sys, time
import os as _os, tempfile as _tempfile
# Paths (see tools/README.md): CURUST_WORK is a scratch folder for builds and results, CURA_ROOT an
# extracted UltiMaker Cura 5.12 AppImage.
WORK = _os.environ.get("CURUST_WORK", _os.path.join(_tempfile.gettempdir(), "curust-work"))
CURA_ROOT = _os.environ.get("CURA_ROOT", WORK + "/cura")
REPO = _os.path.abspath(_os.path.join(_os.path.dirname(_os.path.abspath(__file__)), "..", ".."))

HERE = os.path.dirname(os.path.abspath(__file__))
OFFICIAL = os.environ.get("OFFICIAL_ENGINE", REPO + "/tools/oracle/engine_m1.sh")


def body_hash(path):
    lines = open(path, errors="replace").read().splitlines()
    text = "\n".join(l for l in lines if not l.startswith((";GENERATOR.", ";SLICE_UUID")))
    return hashlib.md5(text.encode()).hexdigest()[:8]


def main():
    root = sys.argv[1]
    n = int(sys.argv[2]) if len(sys.argv) > 2 else 8
    for name in sorted(os.listdir(root)):
        d = os.path.join(root, name)
        if not os.path.exists(os.path.join(d, "job.bin")):
            continue
        cli = subprocess.run([sys.executable, os.path.join(HERE, "job_to_cli.py"), os.path.join(d, "job.bin"), d], capture_output=True, text=True).stdout.split()
        out = os.path.join(d, "rerun.gcode")
        t = time.time()
        aslr = []
        for _ in range(n):
            subprocess.run([OFFICIAL, "slice"] + cli + ["-o", out], cwd=d, capture_output=True)
            aslr.append(body_hash(out))
        fixed = []
        for _ in range(max(1, n // 2)):
            subprocess.run(["setarch", "-R", OFFICIAL, "slice"] + cli + ["-o", out], cwd=d, capture_output=True)
            fixed.append(body_hash(out))
        port = body_hash(os.path.join(d, "curust.gcode")) if os.path.exists(os.path.join(d, "curust.gcode")) else "-"
        print(f"{name}: official, {n} identical runs: {' '.join(aslr)}  ({len(set(aslr))} distinct)  |  "
              f"ASLR off: {' '.join(fixed)}  |  curust: {port}  [{time.time() - t:.0f}s]", flush=True)


if __name__ == "__main__":
    main()
