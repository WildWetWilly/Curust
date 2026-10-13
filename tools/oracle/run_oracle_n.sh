#!/bin/bash
# usage: run_oracle_n.sh N jobs.json out.jsonl   (N selects a private HOME so runs can be parallel)
# Paths (see tools/README.md): CURUST_WORK is a scratch folder, CURA_ROOT an extracted Cura 5.12 AppImage.
: "${CURUST_WORK:=${TMPDIR:-/tmp}/curust-work}"
: "${CURA_ROOT:=$CURUST_WORK/cura}"
REPO=$(cd "$(dirname "$0")/../.." && pwd)
R=$CURA_ROOT
PYD=${CURA_PYTHONHOME:?set CURA_PYTHONHOME to a CPython 3.12 installation (Cura 5.12 uses 3.12)}
IN=$(realpath $2)
OUT=$(realpath -m $3)
mkdir -p "$CURUST_WORK/oracle" && cd "$CURUST_WORK/oracle"
H=$CURUST_WORK/oracle/home_$1
rm -rf $H
ORACLE_IN=$IN ORACLE_OUT=$OUT HOME=$H PYTHONHOME=$PYD PYTHONPATH=$R/share/cura/plugins/CuraEngineBackend LD_LIBRARY_PATH=$R:$R/lib:$R/PyQt6/Qt6/lib timeout 7200 $R/curapy "$REPO/tools/oracle/boot.py" > $CURUST_WORK/oracle/log_$1.txt 2>&1
