import sys, os
import os as _os, tempfile as _tempfile
# Paths (see tools/README.md): CURUST_WORK is a scratch folder for builds and results, CURA_ROOT an
# extracted UltiMaker Cura 5.12 AppImage.
WORK = _os.environ.get("CURUST_WORK", _os.path.join(_tempfile.gettempdir(), "curust-work"))
CURA_ROOT = _os.environ.get("CURA_ROOT", WORK + "/cura")
REPO = _os.path.abspath(_os.path.join(_os.path.dirname(_os.path.abspath(__file__)), "..", ".."))
M = WORK + "/curaenv/merged"
R = CURA_ROOT
sys.path.insert(0, M)
os.environ["QT_QPA_PLATFORM"] = "offscreen"
sys.frozen = True
sys.argv = [R + "/curapy", "--headless"] + sys.argv[1:]
import PyQt6.QtCore as QtCore
# Cura iterates the container ids of its providers as a Python set, so the registry order (and with
# it every "first match" choice) depends on the per-process string hash seed. Make it deterministic:
# iterate the sets of the ContainerRegistry module in sorted order.
import UM.Settings.ContainerRegistry as _cr_mod
class _SortedSet(set):
    def __iter__(self):
        items = list(set.__iter__(self))
        try:
            items.sort()
        except TypeError:
            pass
        return iter(items)
_cr_mod.set = _SortedSet
from cura.CuraApplication import CuraApplication
from UM.View.GL.OpenGLContext import OpenGLContext
from PyQt6.QtGui import QSurfaceFormat
OpenGLContext.detectBestOpenGLVersion = staticmethod(lambda *a, **k: (4, 1, QSurfaceFormat.OpenGLContextProfile.CoreProfile))

def oracle_main():
    try:
        import oracle_task
        oracle_task.run(CuraApplication.getInstance())
    except BaseException:
        import traceback; traceback.print_exc()
    sys.stdout.flush(); sys.stderr.flush()
    os._exit(0)

def patched_exec(self):
    QtCore.QTimer.singleShot(1500, oracle_main)
    return QtCore.QCoreApplication.exec()
CuraApplication.exec = patched_exec
app = CuraApplication()
app.run()
