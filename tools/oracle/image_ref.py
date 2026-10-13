"""Reference for image_check.py: QImage decoding, QImage.scaled and Cura's ImageReader meshes.
Run with Cura's bundled Python (see curapy.sh): image_ref.py CORPUS_DIR OUT_DIR"""
import sys, os, json, struct
from PyQt6.QtGui import QImage, QGuiApplication
from PyQt6.QtCore import Qt
import os as _os, tempfile as _tempfile
# Paths (see tools/README.md): CURUST_WORK is a scratch folder for builds and results, CURA_ROOT an
# extracted UltiMaker Cura 5.12 AppImage.
WORK = _os.environ.get("CURUST_WORK", _os.path.join(_tempfile.gettempdir(), "curust-work"))
CURA_ROOT = _os.environ.get("CURA_ROOT", WORK + "/cura")
REPO = _os.path.abspath(_os.path.join(_os.path.dirname(_os.path.abspath(__file__)), "..", ".."))
app = QGuiApplication(["x", "-platform", "offscreen"])
sys.path.insert(0, CURA_ROOT + "/share/cura/plugins")
import cura.CuraApplication
import ImageReader.ImageReader as irm
from UM.Scene.SceneNode import SceneNode as PlainNode
irm.SceneNode = PlainNode
reader = irm.ImageReader()
src, out = sys.argv[1], sys.argv[2]
params = [
    {"size": 120, "peak": 2.5, "base": 0.4, "smoothing": 1, "lighter_is_higher": False, "transparency": True, "transmittance": 50},
    {"size": 80, "peak": 3.0, "base": 1.0, "smoothing": 3, "lighter_is_higher": True, "transparency": False, "transmittance": 50},
]
summary = {}
for name in sorted(os.listdir(src)):
    path = os.path.join(src, name)
    d = os.path.join(out, name)
    os.makedirs(d, exist_ok=True)
    img = QImage(path)
    info = {"null": img.isNull()}
    if not img.isNull():
        w, h = img.width(), img.height()
        info.update(format=int(img.format().value), width=w, height=h, alpha=img.hasAlphaChannel())
        open(os.path.join(d, "pixels.u32"), "wb").write(struct.pack("<%dI" % (w * h), *[img.pixel(x, y) for y in range(h) for x in range(w)]))
        sizes = [(max(round(w * 512 / max(w, h)), 2), max(round(h * 512 / max(w, h)), 2)), (max(w // 3, 2), max(h // 2, 2))]
        info["scaled"] = []
        for (tw, th) in sizes:
            s = img.scaled(tw, th, Qt.AspectRatioMode.IgnoreAspectRatio)
            open(os.path.join(d, "scaled_%dx%d.u32" % (tw, th)), "wb").write(struct.pack("<%dI" % (tw * th), *[s.pixel(x, y) for y in range(th) for x in range(tw)]))
            info["scaled"].append([tw, th, int(s.format().value), s.hasAlphaChannel()])
        for i, p in enumerate(params):
            node = reader._generateSceneNode(path, p["size"], p["peak"], p["base"], p["smoothing"], 512, p["lighter_is_higher"], p["transparency"], p["transmittance"])
            md = node.getMeshData()
            v = md.getVertices(); idx = md.getIndices()
            flat = v[idx.flatten()] if idx is not None else v
            open(os.path.join(d, "mesh%d.f32" % i), "wb").write(flat.astype("float32").tobytes())
    summary[name] = info
    json.dump(info, open(os.path.join(d, "info.json"), "w"))
json.dump(summary, open(os.path.join(out, "summary.json"), "w"), indent=1)
for k, v in summary.items(): print(k, v.get("format"), v.get("width"), v.get("height"), v.get("alpha"))
