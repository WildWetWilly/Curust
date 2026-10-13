#!/usr/bin/env python3
"""Generate simple binary STL test models into tests/models/."""
import math
import os
import struct
import sys

OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "..", "tests", "models")


def write_stl(path, tris):
    with open(path, "wb") as f:
        f.write(b"curust test model".ljust(80, b"\0"))
        f.write(struct.pack("<I", len(tris)))
        for a, b, c in tris:
            ux, uy, uz = b[0] - a[0], b[1] - a[1], b[2] - a[2]
            vx, vy, vz = c[0] - a[0], c[1] - a[1], c[2] - a[2]
            n = (uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx)
            l = math.sqrt(sum(x * x for x in n)) or 1.0
            f.write(struct.pack("<3f", *(x / l for x in n)))
            for p in (a, b, c):
                f.write(struct.pack("<3f", *p))
            f.write(b"\0\0")


def box(x0, y0, z0, x1, y1, z1):
    v = [(x0, y0, z0), (x1, y0, z0), (x1, y1, z0), (x0, y1, z0), (x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1)]
    faces = [(0, 2, 1), (0, 3, 2), (4, 5, 6), (4, 6, 7), (0, 1, 5), (0, 5, 4), (1, 2, 6), (1, 6, 5), (2, 3, 7), (2, 7, 6), (3, 0, 4), (3, 4, 7)]
    return [(v[a], v[b], v[c]) for a, b, c in faces]


def tube(r_out, r_in, h, n=64, z0=0.0):
    tris = []
    for i in range(n):
        a0 = 2 * math.pi * i / n
        a1 = 2 * math.pi * (i + 1) / n
        po0 = (r_out * math.cos(a0), r_out * math.sin(a0))
        po1 = (r_out * math.cos(a1), r_out * math.sin(a1))
        # outer wall
        tris.append(((po0[0], po0[1], z0), (po1[0], po1[1], z0), (po1[0], po1[1], z0 + h)))
        tris.append(((po0[0], po0[1], z0), (po1[0], po1[1], z0 + h), (po0[0], po0[1], z0 + h)))
        if r_in > 0:
            pi0 = (r_in * math.cos(a0), r_in * math.sin(a0))
            pi1 = (r_in * math.cos(a1), r_in * math.sin(a1))
            tris.append(((pi1[0], pi1[1], z0), (pi0[0], pi0[1], z0), (pi0[0], pi0[1], z0 + h)))
            tris.append(((pi1[0], pi1[1], z0), (pi0[0], pi0[1], z0 + h), (pi1[0], pi1[1], z0 + h)))
            # bottom ring
            tris.append(((po0[0], po0[1], z0), (pi1[0], pi1[1], z0), (po1[0], po1[1], z0)))
            tris.append(((po0[0], po0[1], z0), (pi0[0], pi0[1], z0), (pi1[0], pi1[1], z0)))
            # top ring
            tris.append(((po0[0], po0[1], z0 + h), (po1[0], po1[1], z0 + h), (pi1[0], pi1[1], z0 + h)))
            tris.append(((po0[0], po0[1], z0 + h), (pi1[0], pi1[1], z0 + h), (pi0[0], pi0[1], z0 + h)))
        else:
            tris.append(((0, 0, z0), (po1[0], po1[1], z0), (po0[0], po0[1], z0)))
            tris.append(((0, 0, z0 + h), (po0[0], po0[1], z0 + h), (po1[0], po1[1], z0 + h)))
    return tris


def sphere(r, nu=48, nv=24, center=(0, 0, 0)):
    cx, cy, cz = center
    def p(u, v):
        th = 2 * math.pi * u / nu
        ph = math.pi * v / nv
        return (cx + r * math.sin(ph) * math.cos(th), cy + r * math.sin(ph) * math.sin(th), cz - r * math.cos(ph))
    tris = []
    for v in range(nv):
        for u in range(nu):
            a, b, c, d = p(u, v), p(u + 1, v), p(u + 1, v + 1), p(u, v + 1)
            if v != 0:
                tris.append((a, b, c))
            if v != nv - 1:
                tris.append((a, c, d))
    return tris


def main():
    os.makedirs(OUT, exist_ok=True)
    write_stl(os.path.join(OUT, "cube20.stl"), box(-10, -10, 0, 10, 10, 20))
    write_stl(os.path.join(OUT, "tube.stl"), tube(15, 8, 12))
    write_stl(os.path.join(OUT, "sphere.stl"), sphere(12, center=(0, 0, 12)))
    # A "T" shape: a post with a wide overhanging plate on top (needs support).
    write_stl(os.path.join(OUT, "overhang_t.stl"), box(-4, -4, 0, 4, 4, 10) + box(-20, -8, 10, 20, 8, 14))
    # Thin walls of different widths (exercises Arachne variable width).
    tris = []
    for i, w in enumerate([0.3, 0.5, 0.8, 1.2, 2.0]):
        x = -20 + i * 8
        tris += box(x, -10, 0, x + w, 10, 8)
    write_stl(os.path.join(OUT, "thin_walls.stl"), tris)


def grid_surface(f, nu, nv, closed_u=True):
    """Triangulate a parametric surface f(u, v) with u in [0, nu], v in [0, nv]."""
    tris = []
    for v in range(nv):
        for u in range(nu):
            a, b, c, d = f(u, v), f(u + 1, v), f(u + 1, v + 1), f(u, v + 1)
            tris.append((a, b, c))
            tris.append((a, c, d))
    return tris


def blob(r=14, nu=120, nv=60):
    """A bumpy sphere: many vertices, varying slopes and overhangs."""
    def p(u, v):
        th = 2 * math.pi * u / nu
        ph = math.pi * v / nv
        rr = r * (1 + 0.15 * math.sin(5 * th) * math.sin(3 * ph) + 0.08 * math.cos(7 * ph))
        return (rr * math.sin(ph) * math.cos(th), rr * math.sin(ph) * math.sin(th), r * 1.1 - rr * math.cos(ph))
    tris = []
    for v in range(nv):
        for u in range(nu):
            a, b, c, d = p(u, v), p(u + 1, v), p(u + 1, v + 1), p(u, v + 1)
            if v != 0:
                tris.append((a, b, c))
            if v != nv - 1:
                tris.append((a, c, d))
    return tris


def twisted_star(points=6, r_out=15, r_in=8, hole=4, h=25, layers=50, twist=math.pi / 3):
    """A twisted star prism with a round hole through it."""
    n = points * 2
    def outer(i, k):
        z = h * k / layers
        a = 2 * math.pi * i / n + twist * k / layers
        rr = r_out if i % 2 == 0 else r_in
        return (rr * math.cos(a), rr * math.sin(a), z)
    m = 32
    def inner(i, k):
        z = h * k / layers
        a = 2 * math.pi * i / m
        return (hole * math.cos(a), hole * math.sin(a), z)
    tris = []
    for k in range(layers):
        for i in range(n):
            a, b, c, d = outer(i, k), outer(i + 1, k), outer(i + 1, k + 1), outer(i, k + 1)
            tris.append((a, b, c))
            tris.append((a, c, d))
        for i in range(m):
            a, b, c, d = inner(i, k), inner(i + 1, k), inner(i + 1, k + 1), inner(i, k + 1)
            tris.append((b, a, d))
            tris.append((b, d, c))
    # caps: fan between star and hole (triangulate by angle sweep)
    for k, flip in ((0, True), (layers, False)):
        z = h * k / layers
        ring_o = [outer(i, k) for i in range(n)]
        ring_i = [inner(i, k) for i in range(m)]
        def ang(p):
            return math.atan2(p[1], p[0]) % (2 * math.pi)
        oi = sorted(range(n), key=lambda i: ang(ring_o[i]))
        ii = sorted(range(m), key=lambda i: ang(ring_i[i]))
        # merge both rings by angle and build a strip
        events = [(ang(ring_o[i]), 'o', i) for i in range(n)] + [(ang(ring_i[i]), 'i', i) for i in range(m)]
        events.sort()
        last_o = ring_o[oi[-1]]
        last_i = ring_i[ii[-1]]
        for _, kind, i in events:
            if kind == 'o':
                t = (last_o, ring_o[i], last_i)
                last_o = ring_o[i]
            else:
                t = (last_i, last_o, ring_i[i])
                last_i = ring_i[i]
            if flip:
                t = (t[0], t[2], t[1])
            tris.append(t)
    return tris


def torus(R=14, r=5, nu=64, nv=32, standing=False):
    def p(u, v):
        th = 2 * math.pi * u / nu
        ph = 2 * math.pi * v / nv
        x = (R + r * math.cos(ph)) * math.cos(th)
        y = (R + r * math.cos(ph)) * math.sin(th)
        z = r * math.sin(ph)
        if standing:
            return (x, z, y + R + r)
        return (x, y, z + r)
    return grid_surface(p, nu, nv)


def open_box():
    """A box with its top face missing (open mesh)."""
    return box(-10, -10, 0, 10, 10, 15)[:2] + box(-10, -10, 0, 10, 10, 15)[4:]


def main_extra():
    write_stl(os.path.join(OUT, "blob.stl"), blob())
    write_stl(os.path.join(OUT, "twisted_star.stl"), twisted_star())
    write_stl(os.path.join(OUT, "torus_flat.stl"), torus())
    write_stl(os.path.join(OUT, "torus_standing.stl"), torus(standing=True))
    write_stl(os.path.join(OUT, "open_box.stl"), open_box())
    # Two overlapping boxes in one mesh (self-intersecting mesh).
    write_stl(os.path.join(OUT, "overlap_boxes.stl"), box(-10, -10, 0, 5, 5, 10) + box(-5, -5, 0, 10, 10, 14))


if __name__ == "__main__":
    main()
    main_extra()
