#!/bin/bash
# The official Windows CuraEngine 5.12 (from UltiMaker-Cura-5.12.0-win64-X64.msi) under Wine,
# single-threaded like engine_m1.sh. The Microsoft C/C++ runtime DLLs that ship with Cura
# (ucrtbase, msvcp140, vcruntime140, concrt140) are used instead of Wine's own, so the maths
# library and STL are the real Windows ones.
# Paths (see tools/README.md): CURUST_WORK is a scratch folder, CURA_ROOT an extracted Cura 5.12 AppImage.
: "${CURUST_WORK:=${TMPDIR:-/tmp}/curust-work}"
: "${CURA_ROOT:=$CURUST_WORK/cura}"
REPO=$(cd "$(dirname "$0")/../.." && pwd)
export WINEPREFIX=${WINEPREFIX:-$CURUST_WORK/wine/prefix} WINEDEBUG=-all DISPLAY=
export WINEDLLOVERRIDES="ucrtbase,msvcp140,msvcp140_1,msvcp140_2,msvcp140_atomic_wait,vcruntime140,vcruntime140_1,concrt140=n,b"
exec ${WINE:-$CURUST_WORK/wine/wine-10.0-amd64/bin/wine64} $CURUST_WORK/wincura/CuraEngine.exe "$@" -m1
