#!/usr/bin/env python3
"""Explain G-code differences between Curust and real Cura: re-run the engine stage alone.

For every job, the Curust front-end dumps the exact engine input (CURUST_DUMP_JOB). That input is
converted to CuraEngine command-line form (job_to_cli.py) and sliced by
  * the official CuraEngine 5.12 binary several times (single-threaded, as the oracle runs it),
  * the deterministic reference build of CuraEngine 5.12,
  * the Curust engine.
The official runs differ only in the length of the output file name. If their G-code differs, the
official engine's result depends on memory layout (heap addresses shift with the length of a
string), so no port can reproduce it byte for byte; the port is then checked against the
deterministic reference build instead.

usage: engine_check.py JOBS.json [--only NAME]
"""
import json, os, subprocess, sys
import os as _os, tempfile as _tempfile
# Paths (see tools/README.md): CURUST_WORK is a scratch folder for builds and results, CURA_ROOT an
# extracted UltiMaker Cura 5.12 AppImage.
WORK = _os.environ.get("CURUST_WORK", _os.path.join(_tempfile.gettempdir(), "curust-work"))
CURA_ROOT = _os.environ.get("CURA_ROOT", WORK + "/cura")
REPO = _os.path.abspath(_os.path.join(_os.path.dirname(_os.path.abspath(__file__)), "..", ".."))

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
ROOT = os.path.abspath(os.path.join(HERE, "../.."))
CURUST = os.path.join(ROOT, "target/release/curust")
OFFICIAL = os.environ.get("OFFICIAL_ENGINE", REPO + "/tools/oracle/engine_m1.sh")
REF = os.environ.get("REF_ENGINE", WORK + "/refbuild/curaengine_ref")
OUT = os.path.abspath("engine_check")
PADDINGS = [0, 9, 26, 57, 130, 333, 400, 1001]


def body(path):
    if not os.path.exists(path):
        return None
    # The Griffin header carries each binary's build date / version.
    return [l for l in open(path, errors="replace").read().splitlines()
            if not l.startswith((";GENERATOR.", ";SLICE_UUID"))]


def run(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, timeout=1800, **kw)


def main():
    import gcode_compare  # noqa: reuse the argument builder
    jobs = gcode_compare.load_jobs(sys.argv[1])
    if "--only" in sys.argv:
        jobs = [j for j in jobs if j["name"] == sys.argv[sys.argv.index("--only") + 1]]
    os.makedirs(OUT, exist_ok=True)
    summary = []
    for j in jobs:
        name = j["name"]
        d = os.path.join(OUT, name)
        os.makedirs(d, exist_ok=True)
        jobfile = os.path.join(d, "job.bin")
        args = gcode_compare.curust_args(j)
        args[args.index("-o") + 1] = os.path.join(d, "front.gcode")
        p = run(args, env=dict(os.environ, CURUST_DUMP_JOB=jobfile))
        if p.returncode != 0:
            summary.append((name, "curust failed: " + p.stderr[-200:]))
            continue
        p = run([sys.executable, os.path.join(HERE, "job_to_cli.py"), jobfile, d])
        if p.returncode != 0 or p.stderr.strip():
            summary.append((name, "not convertible: " + p.stderr.strip()[-200:]))
            continue
        cli = p.stdout.split()
        outs = {}
        for label, exe, extra in [("reference", REF, ["-m1"]), ("curust", CURUST, ["-m1"])]:
            o = os.path.join(d, label + ".gcode")
            run([exe, "slice"] + extra + cli + ["-o", o], cwd=d)
            outs[label] = body(o)
        official = []
        for pad in PADDINGS:
            o = os.path.join(d, "official_%d.gcode" % pad)
            # Same file, longer path string ("././././...").
            run([OFFICIAL, "slice"] + cli + ["-o", os.path.join(d, "./" * pad, os.path.basename(o))], cwd=d)
            official.append(body(o))
        variants = len({"\n".join(x or []) for x in official})
        verdict = []
        verdict.append("curust == reference: " + ("yes" if outs["curust"] == outs["reference"] and outs["curust"] else "NO"))
        if variants > 1:
            verdict.append(f"official engine LAYOUT-DEPENDENT ({variants} different results from {len(PADDINGS)} output file names)")
        else:
            verdict.append("official engine stable over %d runs" % len(PADDINGS))
        verdict.append("curust == an official run: " + ("yes" if outs["curust"] in official else "no"))
        summary.append((name, "; ".join(verdict)))
        print(name + ": " + summary[-1][1], flush=True)
    print()
    for n, v in summary:
        print(f"{n}: {v}")


if __name__ == "__main__":
    main()
