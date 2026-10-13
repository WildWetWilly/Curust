import sys, subprocess, re, glob
import os as _os, tempfile as _tempfile
# Paths (see tools/README.md): CURUST_WORK is a scratch folder for builds and results, CURA_ROOT an
# extracted UltiMaker Cura 5.12 AppImage.
WORK = _os.environ.get("CURUST_WORK", _os.path.join(_tempfile.gettempdir(), "curust-work"))
CURA_ROOT = _os.environ.get("CURA_ROOT", WORK + "/cura")
REPO = _os.path.abspath(_os.path.join(_os.path.dirname(_os.path.abspath(__file__)), "..", "..", ".."))
# reuse fuzz generator by running fuzz.py logic: simplest is to regenerate inputs via a modified copy
src = open(_os.path.join(_os.path.dirname(_os.path.abspath(__file__)), 'fuzz.py')).read()
src = src.replace('a = subprocess.run([CPP], input=inp, capture_output=True, text=True, timeout=60)', 'a = None')
src = src.replace('b = subprocess.run([RS], input=inp, capture_output=True, text=True, timeout=60)', 'b = subprocess.run([RS], input=inp, capture_output=True, text=True, timeout=60); m = re.search(r"MP \\[(.*)\\]", b.stderr); vals = [int(x) for x in m.group(1).split(",")]; TOT[:] = [t + v for t, v in zip(TOT, vals)]')
src = src.replace('if a.stdout != b.stdout or a.returncode != b.returncode:', 'if False:')
src = "import re\nTOT=[0]*6\n" + src + "\nprint('TOT', TOT)\n"
exec(compile(src, 'fz', 'exec'))
