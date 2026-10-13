import random, subprocess, sys
import os as _os, tempfile as _tempfile
# Paths (see tools/README.md): CURUST_WORK is a scratch folder for builds and results, CURA_ROOT an
# extracted UltiMaker Cura 5.12 AppImage.
WORK = _os.environ.get("CURUST_WORK", _os.path.join(_tempfile.gettempdir(), "curust-work"))
CURA_ROOT = _os.environ.get("CURA_ROOT", WORK + "/cura")
REPO = _os.path.abspath(_os.path.join(_os.path.dirname(_os.path.abspath(__file__)), "..", "..", ".."))
CPP = WORK + "/wagyu_diff/wdrv"
RS = REPO + "/target/release/examples/wdrv"
seed0 = int(sys.argv[1]) if len(sys.argv) > 1 else 0
N = int(sys.argv[2]) if len(sys.argv) > 2 else 2000
fails = 0
for seed in range(seed0, seed0 + N):
    rnd = random.Random(seed)
    mode = rnd.randrange(4)
    nrings = rnd.randint(1, 6)
    rings = []
    for _ in range(nrings):
        pt = rnd.randrange(2) if mode != 0 else 0
        if mode in (0, 1):
            rng = rnd.choice([3, 5, 10, 30])
            n = rnd.randint(3, 12)
            pts = [(rnd.randint(-rng, rng), rnd.randint(-rng, rng)) for _ in range(n)]
        elif mode == 2:
            # star / circle-like big coords
            import math
            cx, cy = rnd.randint(-1000, 1000), rnd.randint(-1000, 1000)
            n = rnd.randint(3, 40)
            pts = []
            for i in range(n):
                a = 2 * math.pi * i / n
                r = rnd.randint(100, 2000)
                pts.append((cx + int(r * math.cos(a)), cy + int(r * math.sin(a))))
        else:
            # axis-aligned rectangles and rect-ish rings with lots of horizontals
            x0, y0 = rnd.randint(-20, 20), rnd.randint(-20, 20)
            w, h = rnd.randint(0, 20), rnd.randint(0, 20)
            pts = [(x0, y0), (x0 + w, y0), (x0 + w, y0 + h), (x0, y0 + h)]
            if rnd.random() < 0.5:
                pts.append(pts[0])
            if rnd.random() < 0.3:
                pts.reverse()
        rings.append((pt, pts))
    inp = "%d\n" % len(rings) + "".join("%d %d %s\n" % (pt, len(p), " ".join("%d %d" % q for q in p)) for pt, p in rings)
    ct = rnd.randrange(4) if mode != 0 else 1
    ft = rnd.randrange(4) if mode != 0 else 0
    args = [str(ct), str(ft)]
    a = subprocess.run([CPP] + args, input=inp, capture_output=True, text=True, timeout=20)
    b = subprocess.run([RS] + args, input=inp, capture_output=True, text=True, timeout=20)
    if a.stdout != b.stdout or a.returncode != b.returncode:
        fails += 1
        if fails <= 3:
            print("MISMATCH seed", seed, "args", args)
            print(inp)
            print("C++:", a.stdout[:2000], a.stderr[-500:])
            print("RS :", b.stdout[:2000], b.stderr[-500:])
print("done", N, "fails", fails)
