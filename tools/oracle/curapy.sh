#!/bin/bash
# Run a Python script with Cura's bundled interpreter, numpy and PyQt6.
# Paths (see tools/README.md): CURUST_WORK is a scratch folder, CURA_ROOT an extracted Cura 5.12 AppImage.
: "${CURUST_WORK:=${TMPDIR:-/tmp}/curust-work}"
: "${CURA_ROOT:=$CURUST_WORK/cura}"
REPO=$(cd "$(dirname "$0")/../.." && pwd)
R=$CURA_ROOT
PYD=${CURA_PYTHONHOME:?set CURA_PYTHONHOME to a CPython 3.12 installation (Cura 5.12 uses 3.12)}
QT_QPA_PLATFORM=offscreen PYTHONHOME=$PYD PYTHONPATH=$CURUST_WORK/curaenv/merged:$R LD_LIBRARY_PATH=$R:$R/lib:$R/PyQt6/Qt6/lib $R/curapy "$@"
