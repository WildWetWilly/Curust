Painted test models (binary STL + `.uv` + `.png` with a `Description` paint manifest), generated
from the plain models one directory up with `tools/difftest/make_painted_model.py`:

```
G=../../tools/difftest/make_painted_model.py
python3 $G cube20.stl painted/cube_top --proj planar --feature extruder=0,1 --paint "{'extruder': 1 if v > 0.6 else 0}"
python3 $G cube20.stl painted/cube_all1 --proj planar --size 8x8 --feature extruder=0,1 --paint "{'extruder': 1}"
python3 $G cube20.stl painted/cube_none --proj planar --size 8x8 --feature extruder=0,1 --paint "{'extruder': 0}"
python3 $G sphere.stl painted/sphere_stripe --proj cyl --size 64x32 --feature extruder=0,1 --paint "{'extruder': 1 if 0.1 < u < 0.4 else 0}"
python3 $G sphere.stl painted/sphere_seam --proj cyl --size 128x16 --color rgba --feature seam=0,1 --paint "{'seam': 1 if 0.60 < u < 0.64 else (2 if 0.2 < u < 0.5 else 0)}"
python3 $G sphere.stl painted/sphere_gamma --proj cyl --size 64x32 --gamma 100000 --feature raw=0,7 --manifest '{"extruder": [0, 1]}' --paint "{'raw': 1 if u < 0.3 else (2 if u < 0.6 else 0)}"
python3 $G torus_standing.stl painted/torus_multi --proj sphere --size 96x48 --color rgba --feature seam=0,1 --feature extruder=8,10 --paint "{'extruder': 1 if u < 0.3 else (2 if u < 0.45 else 0), 'seam': 1 if 0.7 < u < 0.75 else 0}"
python3 $G torus_flat.stl painted/torus_gray --proj sphere --size 32x32 --color gray --feature extruder=0,0 --paint "{'extruder': (int(u*8)+int(v*8))%2}"
python3 $G overhang_t.stl painted/ot_ga --proj planar --size 32x32 --color ga --feature extruder=4,5 --paint "{'extruder': 0 if v < 0.5 else 1}"
python3 $G tube.stl painted/tube_seam --proj cyl --size 64x8 --color rgb --feature seam=0,1 --paint "{'seam': 1 if 0.3 < u < 0.35 else 0}"
```
