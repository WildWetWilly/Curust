#!/usr/bin/env python3
"""Differential fuzzer: C++ ClipperLib 6.4.2 vs the Rust port.

Usage: fuzz_clipper.py <cpp_driver> <rust_driver> [iterations] [seed]
Both drivers speak the same line protocol; outputs must be byte-identical.
"""
import random
import subprocess
import sys


def rand_poly(rng, n, lo, hi, grid):
    pts = []
    for _ in range(n):
        x = rng.randint(lo, hi) // grid * grid
        y = rng.randint(lo, hi) // grid * grid
        pts.append((x, y))
    # sometimes insert duplicates / collinear points
    if rng.random() < 0.2 and pts:
        i = rng.randrange(len(pts))
        pts.insert(i, pts[i])
    if rng.random() < 0.2 and len(pts) >= 2:
        i = rng.randrange(len(pts) - 1)
        a, b = pts[i], pts[i + 1]
        pts.insert(i + 1, ((a[0] + b[0]) // 2, (a[1] + b[1]) // 2))
    return pts


def rect(rng, lo, hi):
    x0, x1 = sorted(rng.randint(lo, hi) for _ in range(2))
    y0, y1 = sorted(rng.randint(lo, hi) for _ in range(2))
    pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    if rng.random() < 0.5:
        pts.reverse()
    return pts


def fmt(pts):
    return f"{len(pts)} " + " ".join(f"{x} {y}" for x, y in pts)


def gen_case(rng):
    mode = rng.choices(["clip", "offset", "clean", "sort", "heap"], [6, 4, 1, 1, 1])[0]
    scale = rng.choice([10, 100, 1000, 100000, 10000000])
    grid = rng.choice([1, 1, 1, scale // 10 or 1])
    lo, hi = -scale, scale
    lines = []
    if mode == "clip":
        ct = rng.randrange(4)
        sf, cf = rng.randrange(4), rng.randrange(4)
        strict = int(rng.random() < 0.3)
        tree = int(rng.random() < 0.5)
        preserve = int(rng.random() < 0.2)
        rev = int(rng.random() < 0.2)
        nsubj = rng.randint(1, 5)
        nclip = rng.randint(0, 4)
        has_open = False
        body = []
        for _ in range(nsubj):
            closed = 1
            if tree and rng.random() < 0.3:
                closed = 0
                has_open = True
            p = rect(rng, lo, hi) if rng.random() < 0.3 else rand_poly(rng, rng.randint(2, 12), lo, hi, grid)
            body.append(f"S {closed} {fmt(p)}")
        for _ in range(nclip):
            p = rect(rng, lo, hi) if rng.random() < 0.3 else rand_poly(rng, rng.randint(3, 12), lo, hi, grid)
            body.append(f"C 1 {fmt(p)}")
        lines.append(f"clip {ct} {sf} {cf} {strict} {tree} {preserve} {rev}")
        lines += body
        lines.append("END")
    elif mode == "offset":
        delta = rng.choice([1, -1]) * rng.choice([0.0, 0.3, 1, 5, 17.5, 100, 400, 1234.5]) * (scale / 1000 if scale > 1000 else 1)
        ml = rng.choice([1.2, 2.0, 3.0, 10.0])
        at = rng.choice([0.0, 0.25, 1.0, 5.0, 10.0])
        tree = int(rng.random() < 0.4)
        lines.append(f"offset {delta} {ml} {at} {tree}")
        for _ in range(rng.randint(1, 4)):
            jt = rng.randrange(3)
            et = rng.choice([0, 0, 0, 1, 2, 3, 4])
            p = rect(rng, lo, hi) if rng.random() < 0.3 else rand_poly(rng, rng.randint(1, 10), lo, hi, grid)
            lines.append(f"P {jt} {et} {fmt(p)}")
        lines.append("END")
    elif mode == "clean":
        lines.append(f"clean {rng.choice([1.415, 5, 50, 500])}")
        lines.append(f"P {fmt(rand_poly(rng, rng.randint(1, 20), lo, hi, grid))}")
        lines.append("END")
    else:
        n = rng.randint(0, 200)
        keys = [rng.randint(0, rng.choice([3, 10, 1000])) for _ in range(n)]
        lines.append(f"{mode} {n} " + " ".join(map(str, keys)))
    return "\n".join(lines) + "\n"


def read_result(proc):
    out = []
    while True:
        line = proc.stdout.readline()
        if not line:
            raise RuntimeError("driver died")
        if line.strip() == "DONE":
            return "".join(out)
        out.append(line)


def main():
    cpp, rust = sys.argv[1], sys.argv[2]
    iters = int(sys.argv[3]) if len(sys.argv) > 3 else 2000
    seed = int(sys.argv[4]) if len(sys.argv) > 4 else 1
    rng = random.Random(seed)
    pc = subprocess.Popen([cpp], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
    pr = subprocess.Popen([rust], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
    fails = 0
    for i in range(iters):
        case = gen_case(rng)
        pc.stdin.write(case); pc.stdin.flush()
        pr.stdin.write(case); pr.stdin.flush()
        try:
            a = read_result(pc)
        except RuntimeError:
            # C++ crashed (undefined behaviour on degenerate input); restart and skip
            pc = subprocess.Popen([cpp], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
            pr.kill()
            pr = subprocess.Popen([rust], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
            continue
        b = read_result(pr)
        if a != b:
            fails += 1
            if fails <= 3:
                print(f"--- MISMATCH case {i} ---\n{case}--- C++ ---\n{a}--- Rust ---\n{b}")
    print(f"{iters} cases, {fails} mismatches")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
