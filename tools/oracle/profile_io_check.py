#!/usr/bin/env python3
"""Profile import / export against real Cura 5.12.

Exports custom profiles (.curaprofile) from both programs and compares the files byte for byte,
then imports Cura's and Curust's exports, G-code footers and old-version profiles (which go
through Cura's VersionUpgrade plug-ins) into both and compares the resulting quality_changes
containers (ids, names and serialized contents) and the import status.

usage: profile_io_check.py [WORKDIR]     (needs the oracle set-up of oracle/run_oracle_n.sh)
"""
import json, os, subprocess, sys, zipfile, io
from concurrent.futures import ThreadPoolExecutor
import os as _os, tempfile as _tempfile
# Paths (see tools/README.md): CURUST_WORK is a scratch folder for builds and results, CURA_ROOT an
# extracted UltiMaker Cura 5.12 AppImage.
WORK = _os.environ.get("CURUST_WORK", _os.path.join(_tempfile.gettempdir(), "curust-work"))
CURA_ROOT = _os.environ.get("CURA_ROOT", WORK + "/cura")
REPO = _os.path.abspath(_os.path.join(_os.path.dirname(_os.path.abspath(__file__)), "..", ".."))

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "../.."))
ORACLE_DIR = WORK + "/oracle"
CURUST = os.path.join(ROOT, "target/release/curust")
RES = CURA_ROOT + "/share/cura/resources"
W = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "profile_io")
GCODE_DIR = WORK + "/gcmp/gcode_cmp"


def zip_profiles(path, entries):
    b = io.BytesIO()
    with zipfile.ZipFile(b, "w") as z:
        for name, text in entries:
            z.writestr(zipfile.ZipInfo(name), text)
    open(path, "wb").write(b.getvalue())


def make_inputs():
    os.makedirs(W, exist_ok=True)
    # A Cura 4.8-era profile (setting version 16), global container only: per-extruder settings
    # move to the first extruder; old setting names are upgraded.
    old16 = """[general]
version = 4
name = Old Profile
definition = creality_base

[metadata]
type = quality_changes
quality_type = standard
setting_version = 16

[values]
adhesion_type = brim
brim_outside_only = True
bridge_sparse_infill_max_density = 15
infill_sparse_density = 22
outer_inset_first = True
speed_equalize_flow_enabled = False
support_enable = True
support_infill_angle = 45
wall_line_count = 4
wall_overhang_speed_factor = 80
wireframe_enabled = True
"""
    zip_profiles(os.path.join(W, "old16.curaprofile"), [("creality_ender3_old_profile", old16)])
    # A Cura 5.4-era profile with a global and an extruder container.
    g22 = """[general]
version = 4
name = Five Four
definition = creality_base

[metadata]
type = quality_changes
quality_type = super
intent_category = default
setting_version = 22

[values]
support_enable = True
support_structure = tree
"""
    e22 = """[general]
version = 4
name = Five Four
definition = creality_base

[metadata]
type = quality_changes
quality_type = super
intent_category = default
position = 0
setting_version = 22

[values]
infill_pattern = gyroid
support_interface_skip_height = 0.3
wall_thickness = =line_width * 3
"""
    zip_profiles(os.path.join(W, "five_four.curaprofile"), [("creality_ender3_five_four", g22), ("creality_base_extruder_0_#2_five_four", e22)])
    # G-code without a profile footer.
    src = open(os.path.join(GCODE_DIR, "ender_cube.cura.gcode")).read()
    open(os.path.join(W, "no_profile.gcode"), "w").write("".join(l for l in src.splitlines(True) if not l.startswith(";SETTING_3")))


def jobs():
    w = lambda n: os.path.join(W, n)
    export_jobs = []
    for side in ("cura", "curust"):
        export_jobs.append({"name": f"export_ender_{side}", "side": side, "machine": "creality_ender3",
                            "global": {"infill_sparse_density": 37, "support_enable": True, "adhesion_type": "brim"},
                            "extruders": {"0": {"user": {"wall_line_count": 4, "infill_pattern": "gyroid"}}},
                            "custom_profile": "Export Test", "export_profile": w(f"export_ender.{side}.curaprofile")})
        export_jobs.append({"name": f"export_s5_{side}", "side": side, "machine": "ultimaker_s5",
                            "extruders": {"0": {"variant": "AA 0.4", "material": "ultimaker_pla_blue", "user": {"wall_line_count": 3}},
                                          "1": {"variant": "AA 0.4", "material": "ultimaker_pla_red", "user": {"infill_sparse_density": 15}}},
                            "quality_type": "normal", "intent": "engineering",
                            "global": {"layer_height": 0.12},
                            "custom_profile": "Dual Export", "export_profile": w(f"export_s5.{side}.curaprofile")})
    import_jobs = [
        {"name": "import_cura_export", "machine": "creality_ender3", "import_profiles": [w("export_ender.cura.curaprofile")]},
        {"name": "import_curust_export", "machine": "creality_ender3", "import_profiles": [w("export_ender.curust.curaprofile")]},
        {"name": "import_s5_export", "machine": "ultimaker_s5", "import_profiles": [w("export_s5.cura.curaprofile")]},
        {"name": "import_twice", "machine": "creality_ender3", "import_profiles": [w("export_ender.cura.curaprofile"), w("export_ender.cura.curaprofile")]},
        {"name": "import_cross_printer", "machine": "prusa_i3_mk3", "import_profiles": [w("export_ender.cura.curaprofile")]},
        {"name": "import_ender_in_cr10", "machine": "creality_cr10", "import_profiles": [w("export_ender.cura.curaprofile")]},
        {"name": "import_gcode_ender", "machine": "creality_ender3", "import_profiles": [os.path.join(GCODE_DIR, "ender_brim_user.cura.gcode")]},
        {"name": "import_gcode_s5", "machine": "ultimaker_s5", "import_profiles": [os.path.join(GCODE_DIR, "s5_two_extruders.cura.gcode")]},
        {"name": "import_gcode_none", "machine": "creality_ender3", "import_profiles": [w("no_profile.gcode")]},
        {"name": "import_old16", "machine": "creality_ender3", "import_profiles": [w("old16.curaprofile")]},
        {"name": "import_five_four", "machine": "creality_ender3", "import_profiles": [w("five_four.curaprofile")]},
    ]
    for j in import_jobs:
        j["dump_profiles"] = True
    return export_jobs, import_jobs


def run_oracle(job, n, tries=3):
    for _ in range(tries):
        r = run_oracle_once(job, n)
        if "no oracle output" not in r.get("error", ""):
            return r
    return r


def run_oracle_once(job, n):
    j = dict(job)
    j["models"] = []
    jf = os.path.join(W, j["name"] + ".job.json")
    out = os.path.join(W, j["name"] + ".oracle.jsonl")
    json.dump([j], open(jf, "w"))
    subprocess.run(["bash", os.path.join(ORACLE_DIR, "run_oracle_n.sh"), str(n), jf, out])
    try:
        return json.loads(open(out).readline())
    except Exception:
        return {"error": "no oracle output (see %s/log_%d.txt)" % (ORACLE_DIR, n)}


def run_curust(job):
    a = [CURUST, "slice-models", "--resources", RES, "--machine", job["machine"]]
    for pos, ext in sorted(job.get("extruders", {}).items()):
        if "variant" in ext:
            a += ["--variant", f"{pos}={ext['variant']}"]
        if "material" in ext:
            a += ["--material", f"{pos}={ext['material']}"]
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
    if "export_profile" in job:
        a += ["--export-profile", job["export_profile"]]
    for p in job.get("import_profiles", []):
        a += ["--import-profile", p]
    dump = os.path.join(W, job["name"] + ".curust.json")
    if job.get("dump_profiles"):
        a += ["--dump-profiles", dump]
    r = subprocess.run(a, capture_output=True, text=True)
    res = {"stderr": r.stderr, "rc": r.returncode}
    res["import_status"] = [l.split(" ", 2)[2].split(" ")[0] for l in r.stderr.splitlines() if l.startswith("import ")]
    if job.get("dump_profiles") and os.path.exists(dump):
        res["profiles"] = json.load(open(dump))
    return res


def main():
    make_inputs()
    # Cura asks before overwriting an existing file (a modal dialog on Linux): start clean.
    for f in os.listdir(W):
        if f.startswith("export_") and f.endswith(".curaprofile"):
            os.remove(os.path.join(W, f))
    export_jobs, import_jobs = jobs()
    ok = 0
    total = 0
    with ThreadPoolExecutor(4) as ex:
        list(ex.map(lambda p: run_oracle(p[1], 30 + p[0]) if p[1]["side"] == "cura" else run_curust(p[1]), enumerate(export_jobs)))
    for side_pair in (("export_ender.cura.curaprofile", "export_ender.curust.curaprofile"), ("export_s5.cura.curaprofile", "export_s5.curust.curaprofile")):
        total += 1
        a, b = (os.path.join(W, f) for f in side_pair)
        if os.path.exists(a) and os.path.exists(b) and open(a, "rb").read() == open(b, "rb").read():
            ok += 1
            print(f"{side_pair[0]}: IDENTICAL ({os.path.getsize(a)} bytes)")
        else:
            print(f"{side_pair[0]}: DIFFERENT")
            for f in (a, b):
                if os.path.exists(f):
                    with zipfile.ZipFile(f) as z:
                        for n in z.namelist():
                            print("  ", os.path.basename(f), n, repr(z.read(n).decode()[:400]))
    with ThreadPoolExecutor(6) as ex:
        oracle = list(ex.map(lambda p: run_oracle(p[1], 40 + p[0]), enumerate(import_jobs)))
    for job, o in zip(import_jobs, oracle):
        total += 1
        c = run_curust(job)
        cura_status = [r.get("status") for r in o.get("import_results", [])]
        problems = []
        if "error" in o:
            problems.append("oracle error: " + o["error"][-300:])
        if cura_status != c["import_status"]:
            problems.append(f"status cura={cura_status} curust={c['import_status']}")
        cp, rp = o.get("profiles", {}), c.get("profiles", {})
        if cp != rp:
            problems.append("profiles differ")
            for name in sorted(set(cp) | set(rp)):
                a, b = cp.get(name, {}), rp.get(name, {})
                if a != b:
                    problems.append(f"  [{name}] cura ids={sorted(a)} curust ids={sorted(b)}")
                    for cid in sorted(set(a) & set(b)):
                        if a[cid] != b[cid]:
                            problems.append(f"    {cid}:\n      cura:   {a[cid]!r}\n      curust: {b[cid]!r}")
        if problems:
            print(f"{job['name']}: DIFFERENT")
            for p in problems:
                print("  " + p)
            print("   cura messages:", [r.get("message") for r in o.get("import_results", [])])
            print("   curust:", c["stderr"].strip()[:500])
        else:
            ok += 1
            print(f"{job['name']}: IDENTICAL ({', '.join(cura_status)}; {sum(len(v) for v in cp.values())} containers)")
    print(f"{ok}/{total} identical")


if __name__ == "__main__":
    main()
