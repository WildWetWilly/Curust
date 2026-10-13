import random, subprocess, sys, math
import os as _os, tempfile as _tempfile
# Paths (see tools/README.md): CURUST_WORK is a scratch folder for builds and results, CURA_ROOT an
# extracted UltiMaker Cura 5.12 AppImage.
WORK = _os.environ.get("CURUST_WORK", _os.path.join(_tempfile.gettempdir(), "curust-work"))
CURA_ROOT = _os.environ.get("CURA_ROOT", WORK + "/cura")
REPO = _os.path.abspath(_os.path.join(_os.path.dirname(_os.path.abspath(__file__)), "..", "..", ".."))
CPP = WORK + "/vor_diff/vdrv"
RS = REPO + "/target/release/examples/vdrv"
seed0 = int(sys.argv[1]); N = int(sys.argv[2])
fails = 0

def star(rnd, cx, cy, rmin, rmax, n, jitter_angle=True):
    pts = []
    angs = sorted(rnd.uniform(0, 2*math.pi) for _ in range(n)) if jitter_angle else [2*math.pi*i/n for i in range(n)]
    for a in angs:
        r = rnd.uniform(rmin, rmax)
        pts.append((int(round(cx + r*math.cos(a))), int(round(cy + r*math.sin(a)))))
    # dedupe consecutive
    out = []
    for p in pts:
        if not out or out[-1] != p:
            out.append(p)
    if len(out) > 1 and out[0] == out[-1]:
        out.pop()
    return out

def seg_inter(p1, p2, p3, p4):
    # proper or touching intersection test (excluding shared endpoints)
    def orient(a, b, c):
        v = (b[0]-a[0])*(c[1]-a[1]) - (b[1]-a[1])*(c[0]-a[0])
        return (v > 0) - (v < 0)
    def onseg(a, b, c):
        return min(a[0], b[0]) <= c[0] <= max(a[0], b[0]) and min(a[1], b[1]) <= c[1] <= max(a[1], b[1])
    shared = {p1, p2} & {p3, p4}
    o1, o2, o3, o4 = orient(p1, p2, p3), orient(p1, p2, p4), orient(p3, p4, p1), orient(p3, p4, p2)
    if shared:
        # allowed only if not collinear overlapping
        if o1 == 0 and o2 == 0:
            # collinear: overlapping beyond the shared point?
            s = shared.pop()
            other1 = p2 if p1 == s else p1
            other2 = p4 if p3 == s else p3
            dot = (other1[0]-s[0])*(other2[0]-s[0]) + (other1[1]-s[1])*(other2[1]-s[1])
            return dot > 0
        return False
    if o1 != o2 and o3 != o4: return True
    if o1 == 0 and onseg(p1, p2, p3): return True
    if o2 == 0 and onseg(p1, p2, p4): return True
    if o3 == 0 and onseg(p3, p4, p1): return True
    if o4 == 0 and onseg(p3, p4, p2): return True
    return False

def filter_segments(rnd, segs):
    kept = []
    for s in segs:
        if s[0] == s[1]: continue
        if any(seg_inter(s[0], s[1], k[0], k[1]) for k in kept): continue
        kept.append(s)
    return kept

for seed in range(seed0, seed0 + N):
    rnd = random.Random(seed)
    mode = rnd.randrange(6)
    pts = []; segs = []
    if mode == 0:
        rng = rnd.choice([3, 10, 100, 100000])
        pts = [(rnd.randint(-rng, rng), rnd.randint(-rng, rng)) for _ in range(rnd.randint(1, 40))]
    elif mode == 1:
        # star polygons, possibly with holes
        npoly = rnd.randint(1, 3)
        for k in range(npoly):
            cx, cy = k * 5000, rnd.randint(-100, 100)
            outer = star(rnd, cx, cy, 800, 2000, rnd.randint(3, 25))
            polys = [outer]
            if rnd.random() < 0.6:
                hole = star(rnd, cx, cy, 100, 500, rnd.randint(3, 12))
                polys.append(hole[::-1])
            for poly in polys:
                for i in range(len(poly)):
                    segs.append((poly[i], poly[(i+1) % len(poly)]))
        segs = filter_segments(rnd, segs)
    elif mode == 2:
        # random small-grid segments (many degeneracies)
        rng = rnd.choice([4, 8, 20])
        cand = []
        for _ in range(rnd.randint(1, 25)):
            a = (rnd.randint(-rng, rng), rnd.randint(-rng, rng))
            if rnd.random() < 0.3:
                b = (a[0], rnd.randint(-rng, rng))
            elif rnd.random() < 0.3:
                b = (rnd.randint(-rng, rng), a[1])
            else:
                b = (rnd.randint(-rng, rng), rnd.randint(-rng, rng))
            cand.append((a, b))
        segs = filter_segments(rnd, cand)
        if rnd.random() < 0.5:
            pts = [(rnd.randint(-rng, rng), rnd.randint(-rng, rng)) for _ in range(rnd.randint(0, 10))]
    elif mode == 3:
        # axis-aligned rectangles grid (walls-like), nested
        for k in range(rnd.randint(1, 4)):
            x0, y0 = rnd.randint(0, 50) * 100 + k * 10000, rnd.randint(0, 50) * 100
            w, h = rnd.randint(1, 30) * 100, rnd.randint(1, 30) * 100
            rect = [(x0, y0), (x0 + w, y0), (x0 + w, y0 + h), (x0, y0 + h)]
            if rnd.random() < 0.5: rect = rect[::-1]
            for i in range(4):
                segs.append((rect[i], rect[(i+1) % 4]))
            if rnd.random() < 0.5 and w > 200 and h > 200:
                inner = [(x0 + 100, y0 + 100), (x0 + 100, y0 + h - 100), (x0 + w - 100, y0 + h - 100), (x0 + w - 100, y0 + 100)]
                for i in range(4):
                    segs.append((inner[i], inner[(i+1) % 4]))
        segs = filter_segments(rnd, segs)
    elif mode == 4:
        # regular polygons (circles), very cocircular
        n = rnd.randint(3, 60)
        r = rnd.choice([50, 1000, 20000])
        poly = star(rnd, 0, 0, r, r, n, jitter_angle=False)
        for i in range(len(poly)):
            segs.append((poly[i], poly[(i+1) % len(poly)]))
        if rnd.random() < 0.5:
            poly2 = star(rnd, 0, 0, r // 2, r // 2, n, jitter_angle=False)[::-1]
            for i in range(len(poly2)):
                segs.append((poly2[i], poly2[(i+1) % len(poly2)]))
        segs = filter_segments(rnd, segs)
    else:
        # large coordinates near int32 range limits / thin features
        n = rnd.randint(3, 15)
        poly = star(rnd, rnd.randint(-10**8, 10**8), rnd.randint(-10**8, 10**8), 10**5, 10**7, n)
        for i in range(len(poly)):
            segs.append((poly[i], poly[(i+1) % len(poly)]))
        segs = filter_segments(rnd, segs)
        if rnd.random() < 0.5:
            pts = [(rnd.randint(-10**8, 10**8), rnd.randint(-10**8, 10**8)) for _ in range(rnd.randint(0, 10))]
    if not pts and not segs:
        continue
    inp = "%d\n%s%d\n%s" % (len(pts), "".join("%d %d\n" % p for p in pts), len(segs), "".join("%d %d %d %d\n" % (s[0][0], s[0][1], s[1][0], s[1][1]) for s in segs))
    a = subprocess.run([CPP], input=inp, capture_output=True, text=True, timeout=60)
    b = subprocess.run([RS], input=inp, capture_output=True, text=True, timeout=60)
    if a.stdout != b.stdout or a.returncode != b.returncode:
        fails += 1
        if fails <= 2:
            print("MISMATCH seed", seed, "mode", mode)
            open(WORK + "/vfail_%d.txt" % seed, "w").write(inp)
            print("rc", a.returncode, b.returncode, b.stderr[-800:])
print("done", N, "fails", fails)
