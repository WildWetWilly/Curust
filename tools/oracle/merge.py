import os, shutil
import os as _os, tempfile as _tempfile
# Paths (see tools/README.md): CURUST_WORK is a scratch folder for builds and results, CURA_ROOT an
# extracted UltiMaker Cura 5.12 AppImage.
WORK = _os.environ.get("CURUST_WORK", _os.path.join(_tempfile.gettempdir(), "curust-work"))
CURA_ROOT = _os.environ.get("CURA_ROOT", WORK + "/cura")
REPO = _os.path.abspath(_os.path.join(_os.path.dirname(_os.path.abspath(__file__)), "..", ".."))
R = CURA_ROOT
Z = WORK + "/pyx/out/UltiMaker-Cura_extracted/PYZ-00.pyz_extracted"
O = WORK + "/curaenv/merged"
def merge(srcs, out):
    os.makedirs(out, exist_ok=True)
    names = {}
    for s in srcs:
        for n in os.listdir(s):
            names.setdefault(n, []).append(os.path.join(s, n))
    for n, paths in names.items():
        dst = os.path.join(out, n)
        dirs = [p for p in paths if os.path.isdir(p)]
        if len(dirs) >= 2:
            merge(dirs, dst)
        elif not os.path.lexists(dst):
            os.symlink(paths[0], dst)
shutil.rmtree(O, ignore_errors=True)
merge([R, Z], O)
print("ok")
