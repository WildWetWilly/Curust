#!/usr/bin/env python3
"""Compare complete G-code files: real Cura 5.12 (oracle, single-threaded CuraEngine) against the
`curust slice-models` front-end.

usage: gcode_compare.py JOBS.json [--skip-oracle] [--only NAME]

Every job needs explicit model positions (Cura's arrange differs from Curust's). Writes
<name>.cura.gcode and <name>.curust.gcode into ./gcode_cmp/.
"""
import json, os, shlex, shutil, subprocess, sys
import os as _os, tempfile as _tempfile
# Paths (see tools/README.md): CURUST_WORK is a scratch folder for builds and results, CURA_ROOT an
# extracted UltiMaker Cura 5.12 AppImage.
WORK = _os.environ.get("CURUST_WORK", _os.path.join(_tempfile.gettempdir(), "curust-work"))
CURA_ROOT = _os.environ.get("CURA_ROOT", WORK + "/cura")
REPO = _os.path.abspath(_os.path.join(_os.path.dirname(_os.path.abspath(__file__)), "..", ".."))

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "../.."))
ORACLE_DIR = WORK + "/oracle"
CURUST = os.path.join(HERE, "../../target/release/curust")
RES = CURA_ROOT + "/share/cura/resources"
OUT = os.path.abspath("gcode_cmp")

def curust_args(job, index=0):
    # CURUST_CMD runs another build, e.g. "wine target/x86_64-pc-windows-gnullvm/release/curust-cli.exe".
    prefix = shlex.split(os.environ["CURUST_CMD"]) if os.environ.get("CURUST_CMD") else [CURUST]
    a = prefix + ["slice-models", "--resources", RES, "--machine", job["machine"], "--machine-name", "oracle_%d" % index, "--fixed-time"]
    for path in job.get("import_materials", []):
        a += ["--import-material", path]
    for pos, ext in sorted(job.get("extruders", {}).items()):
        if "variant" in ext:
            a += ["--variant", f"{pos}={ext['variant']}"]
        if "material" in ext:
            a += ["--material", f"{pos}={ext['material']}"]
        if ext.get("enabled") is False:
            a += ["--disable-extruder", pos]
    if "quality_type" in job:
        a += ["--quality", job["quality_type"]]
    if "intent" in job:
        a += ["--intent", job["intent"]]
    for k, v in job.get("global", {}).items():
        a += ["--raw-set", f"{k}={json.dumps(v)}"]
    for pos, ext in job.get("extruders", {}).items():
        for k, v in ext.get("user", {}).items():
            a += ["--raw-extruder-set", f"{pos}:{k}={json.dumps(v)}"]
    if "custom_profile" in job:
        a += ["--create-profile", job["custom_profile"]]
    for m in job["models"]:
        x, y = m.get("position", [0, 0])
        a += ["--at", f"{x},{y}"]
        if "extruder" in m:
            a += ["--extruder", str(m["extruder"])]
        for k, v in m.get("settings", {}).items():
            vv = v if isinstance(v, str) else json.dumps(v) if not isinstance(v, bool) else ("True" if v else "False")
            a += ["--object-set", f"{k}={vv}"]
        if "image" in m:
            a += ["--image-options", ",".join(f"{k}={int(v) if isinstance(v, bool) else v}" for k, v in m["image"].items())]
        a.append(m["file"])
        for b in m.get("support_blockers", []):
            a += ["--support-blocker", ",".join(repr(float(c)) for c in b)]
    if job.get("post_processing") is not None:
        meta_path = os.path.join(OUT, job["name"] + ".pp_meta.txt")
        with open(meta_path, "w") as f:
            f.write(job["post_processing"])
        a += ["--post-process", meta_path]
    for key, settings in job.get("add_scripts", []):
        a += ["--add-script-json", json.dumps([key, settings])]
    a += ["-o", os.path.join(OUT, job["name"] + ".curust.gcode")]
    return a

def load_jobs(path):
    """Model paths in a job file are relative to the repository root."""
    jobs = json.load(open(path))
    for j in jobs:
        j["import_materials"] = [os.path.join(ROOT, p) for p in j.get("import_materials", [])]
        for m in j["models"]:
            m["file"] = os.path.join(ROOT, m["file"])
    return jobs

def normalize(text):
    # The slice UUID is random in Cura.
    return [l for l in text.splitlines() if not l.startswith(";SLICE_UUID")]

def main():
    jobs = load_jobs(sys.argv[1])
    only = sys.argv[sys.argv.index("--only") + 1] if "--only" in sys.argv else None
    if only:
        jobs = [j for j in jobs if j["name"] == only]
    os.makedirs(OUT, exist_ok=True)
    for j in jobs:
        j["gcode_out"] = os.path.join(OUT, j["name"] + ".cura.gcode")
    if "--skip-oracle" not in sys.argv:
        jf = os.path.join(OUT, "jobs.json")
        json.dump(jobs, open(jf, "w"))
        env = dict(os.environ, ORACLE_ENGINE=os.environ.get("ORACLE_ENGINE") or os.path.join(REPO, "tools", "oracle", "engine_m1.sh"))
        subprocess.run(["bash", os.path.join(ORACLE_DIR, "run_oracle_n.sh"), os.environ.get("ORACLE_N", "9"), jf, os.path.join(OUT, "oracle.jsonl")], env=env)
    oracle = {}
    if os.path.exists(os.path.join(OUT, "oracle.jsonl")):
        for l in open(os.path.join(OUT, "oracle.jsonl")):
            r = json.loads(l)
            oracle[r["name"]] = r
    ok = 0
    for index, j in enumerate(jobs):
        name = j["name"]
        o = oracle.get(name, {})
        if o.get("error"):
            print(f"{name}: ORACLE ERROR {o['error'][-200:]}")
            continue
        # A fresh configuration folder per job (imported materials go there).
        cfg = os.path.join(OUT, "cfg_" + j["name"])
        shutil.rmtree(cfg, ignore_errors=True)
        meta_out = os.path.join(OUT, name + ".curust_pp_meta.txt")
        p = subprocess.run(curust_args(j, index), capture_output=True, text=True, env=dict(os.environ, CURA_FIXED_UUID="00000000-0000-0000-0000-000000000000", CURUST_CONFIG_DIR=cfg, CURUST_NO_CURA_SCRIPTS="1", CURUST_DUMP_PP_META=meta_out))
        if "pp_meta" in o and os.path.exists(meta_out):
            mine = open(meta_out).read()
            if mine != o["pp_meta"]:
                print(f"{name}: post_processing_scripts metadata differs:\n  cura   {o['pp_meta']!r}\n  curust {mine!r}")
        if p.stderr.strip() and "post-processing" in p.stderr:
            print("  " + "\n  ".join(l for l in p.stderr.splitlines() if "post-processing" in l or "message" in l))
        cura_file = j["gcode_out"]
        rust_file = os.path.join(OUT, name + ".curust.gcode")
        if p.returncode != 0:
            print(f"{name}: CURUST FAILED {p.stderr.strip()[-300:]}  (oracle state {o.get('backend_state')})")
            continue
        a = normalize(open(cura_file).read()) if os.path.exists(cura_file) else []
        b = normalize(open(rust_file).read())
        if a == b and a:
            ok += 1
            print(f"{name}: IDENTICAL ({len(a)} lines)")
        else:
            first = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
            ndiff = sum(1 for x, y in zip(a, b) if x != y) + abs(len(a) - len(b))
            print(f"{name}: DIFF at line {first + 1}: cura={a[first] if first < len(a) else None!r} curust={b[first] if first < len(b) else None!r} ({ndiff} lines differ, {len(a)} vs {len(b)})")
    print(f"{ok}/{len(jobs)} identical")

if __name__ == "__main__":
    main()
