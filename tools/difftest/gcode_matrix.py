#!/usr/bin/env python3
"""Run the reference engine and the Rust port over a matrix of (model, settings) cases and
compare the full G-code output byte for byte.

usage: gcode_matrix.py <cases-file> [-j N] [-k filter-substring]
Each non-empty, non-# line of the cases file is:  name | model(s) | key=value key=value ...
Settings are applied to the scene (global) settings and, when the key exists there, to extruder 0.
`@def=dual` / `@def=<path to a resolved settings json>` picks the -r file; a model list of `-` loads
no models with -l (for resolved files that list models themselves). The engines run in the
repository root.
"""
import shlex, json, os, subprocess, sys, hashlib, concurrent.futures, argparse
import os as _os, tempfile as _tempfile
# Paths (see tools/README.md): CURUST_WORK is a scratch folder for builds and results, CURA_ROOT an
# extracted UltiMaker Cura 5.12 AppImage.
WORK = _os.environ.get("CURUST_WORK", _os.path.join(_tempfile.gettempdir(), "curust-work"))
CURA_ROOT = _os.environ.get("CURA_ROOT", WORK + "/cura")
REPO = _os.path.abspath(_os.path.join(_os.path.dirname(_os.path.abspath(__file__)), "..", ".."))

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
REF = os.environ.get("REF_ENGINE", WORK + "/refbuild/curaengine_ref")
RUST = os.environ.get("RUST_ENGINE", os.path.join(ROOT, "target/release/curust-engine"))
DEF = os.environ.get("DEF_JSON", WORK + "/ender3.json")
DEFS = {"ender3": DEF, "dual": os.path.join(ROOT, "tools/difftest/ender3_dual_resolved.json")}
OUT = os.environ.get("CMP_DIR", WORK + "/gm")

ext_keys = set(json.load(open(DEF))["extruder.0"].keys())

def build_args(models, settings):
    definition = DEF
    for k, v in settings:
        if k == "@def":
            definition = DEFS[v] if v in DEFS else (v if os.path.isabs(v) else os.path.join(ROOT, v))
    settings = [(k, v) for k, v in settings if not k.startswith("@")]
    a = ["slice", "-m1", "-r", definition]  # -m1: the reference is only deterministic single-threaded (rand() order)
    for k, v in settings:
        a += ["-s", f"{k}={v}"]
    ek = [(k, v) for k, v in settings if k in ext_keys]
    n_extruders = len([k for k in json.load(open(definition)) if k.startswith("extruder.")])
    for e in range(n_extruders - 1, -1, -1):
        a += [f"-e{e}"]
        for k, v in ek:
            a += ["-s", f"{k}={v}"]
    for m in models:
        if m == "-":  # no -l models (the resolved settings file lists them)
            continue
        # model token: file.stl[@extruder][[key=value,key=value]]
        mesh_settings = []
        if m.endswith("]") and "[" in m:
            m, extra = m[:-1].split("[", 1)
            mesh_settings = [kv.split("=", 1) for kv in extra.split(",") if kv]
        extruder = None
        if "@" in m:
            m, extruder = m.split("@", 1)
            a += ["-e" + extruder]
        a += ["-l", m if os.path.isabs(m) else os.path.join(ROOT, "tests/models", m)]
        if extruder is not None:
            a += ["-s", f"extruder_nr={extruder}"]
        for k, v in mesh_settings:
            a += ["-s", f"{k}={v}"]
    return a

def run(case):
    name, models, settings = case
    hw = os.environ.get("CURA_HW_CONCURRENCY", "2")
    for k, v in settings:
        if k == "@hw":  # per-case std::thread::hardware_concurrency() override (tree support bucketing)
            hw = v
    args = build_args(models, settings)
    ref_out = os.path.join(OUT, f"ref_{name}.gcode")
    rust_out = os.path.join(OUT, f"rust_{name}.gcode")
    def go(exe, out):
        try:
            env = dict(os.environ, CURA_FIXED_UUID="00000000-0000-4000-8000-000000000000", CURA_HW_CONCURRENCY=hw)
            p = subprocess.run([exe] + args + ["-o", out], capture_output=True, timeout=900, env=env, cwd=ROOT)
            return p.returncode, p.stderr.decode(errors="replace")
        except subprocess.TimeoutExpired:
            return -999, "TIMEOUT"
    rr, _ = go(REF, ref_out)
    rc, err = go(RUST, rust_out)
    a = open(ref_out, "rb").read() if os.path.exists(ref_out) else b""
    b = open(rust_out, "rb").read() if os.path.exists(rust_out) else b""
    if rr != 0:
        return name, "REFFAIL", f"ref rc={rr}"
    # REF_REPEAT=N: run the reference N-1 more times (with longer output paths, which shifts its heap
    # layout); if any run differs, the reference itself is nondeterministic for this case.
    variants = {a}
    for k in range(1, int(os.environ.get("REF_REPEAT", "1"))):
        o2 = os.path.join(OUT, "./" * (7 * k), f"ref{k}_{name}.gcode")
        go(REF, o2)
        variants.add(open(o2, "rb").read() if os.path.exists(o2) else b"")
    if len(variants) > 1:
        return name, "REFNONDET" + (" (matches one run)" if b in variants else ""), f"{len(variants)} different results from {os.environ.get('REF_REPEAT')} runs"
    # The Griffin header carries the engine's build date (the reference's compile date).
    import re as _re
    a = _re.sub(rb";GENERATOR\.BUILD_DATE:[^\n]*", b";GENERATOR.BUILD_DATE:", a)
    b = _re.sub(rb";GENERATOR\.BUILD_DATE:[^\n]*", b";GENERATOR.BUILD_DATE:", b)
    if a == b:
        return name, "OK", f"{a.count(b'\n')} lines {hashlib.md5(a).hexdigest()[:8]}"
    msg = ""
    for line in err.splitlines():
        if "panicked" in line or "not implemented" in line or "error" in line.lower():
            msg = line.strip()[:200]
            break
    if rc != 0:
        return name, "CRASH", f"rc={rc} {msg}"
    la, lb = a.split(b"\n"), b.split(b"\n")
    first = next((i for i in range(min(len(la), len(lb))) if la[i] != lb[i]), min(len(la), len(lb)))
    return name, "DIFF", f"line {first+1}: ref={la[first][:70] if first < len(la) else b'EOF'} rust={lb[first][:70] if first < len(lb) else b'EOF'}"

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cases")
    ap.add_argument("-j", type=int, default=2)
    ap.add_argument("-k", default=None)
    o = ap.parse_args()
    os.makedirs(OUT, exist_ok=True)
    cases = []
    for line in open(o.cases):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = [p.strip() for p in line.split("|")]
        name, models = parts[0], parts[1].split()
        settings = [tuple(kv.replace("{ROOT}", ROOT).split("=", 1)) for kv in (shlex.split(parts[2]) if len(parts) > 2 else [])]
        if o.k and o.k not in name:
            continue
        cases.append((name, models, settings))
    counts = {}
    with concurrent.futures.ThreadPoolExecutor(o.j) as ex:
        for name, status, info in ex.map(run, cases):
            counts[status] = counts.get(status, 0) + 1
            print(f"{status:8} {name:40} {info}", flush=True)
    print(" ".join(f"{k}={v}" for k, v in sorted(counts.items())))

if __name__ == "__main__":
    main()
