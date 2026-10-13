#!/bin/bash
# usage: run_oracle.sh jobs.json out.jsonl
# Paths (see tools/README.md): CURUST_WORK is a scratch folder, CURA_ROOT an extracted Cura 5.12 AppImage.
: "${CURUST_WORK:=${TMPDIR:-/tmp}/curust-work}"
: "${CURA_ROOT:=$CURUST_WORK/cura}"
REPO=$(cd "$(dirname "$0")/../.." && pwd)
R=$CURA_ROOT
PYD=${CURA_PYTHONHOME:?set CURA_PYTHONHOME to a CPython 3.12 installation (Cura 5.12 uses 3.12)}
mkdir -p "$CURUST_WORK/oracle" && cd "$CURUST_WORK/oracle"
rm -rf $CURUST_WORK/oracle/home
ORACLE_IN=$(realpath $1) ORACLE_OUT=$(realpath -m $2) HOME=$CURUST_WORK/oracle/home PYTHONHOME=$PYD PYTHONPATH=$R/share/cura/plugins/CuraEngineBackend LD_LIBRARY_PATH=$R:$R/lib:$R/PyQt6/Qt6/lib timeout 3000 $R/curapy "$REPO/tools/oracle/boot.py" > $CURUST_WORK/oracle/log.txt 2>&1
