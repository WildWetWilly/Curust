#!/bin/bash
# Compare full G-code output of the patched reference CuraEngine and the Rust port.
# usage: cmp_gcode.sh <model.stl> [global args...] [-- mesh args...]
# Paths (see tools/README.md): CURUST_WORK is a scratch folder, CURA_ROOT an extracted Cura 5.12 AppImage.
: "${CURUST_WORK:=${TMPDIR:-/tmp}/curust-work}"
: "${CURA_ROOT:=$CURUST_WORK/cura}"
REPO=$(cd "$(dirname "$0")/../.." && pwd)
model=$1; shift
G=(); M=(); seen=0
for a in "$@"; do if [ "$a" == "--" ]; then seen=1; elif [ $seen == 0 ]; then G+=("$a"); else M+=("$a"); fi; done
T=${CMP_DIR:-$CURUST_WORK/g}; mkdir -p $T
name=${CMP_NAME:-$(basename $model .stl)}
REF=${REF_ENGINE:-$CURUST_WORK/refbuild/curaengine_ref}
RUST=${RUST_ENGINE:-$REPO/target/release/curust-engine}
DEF=${DEF_JSON:-$CURUST_WORK/ender3.json}
timeout 600 $REF slice -r $DEF "${G[@]}" -l $model "${M[@]}" -o $T/ref_$name.gcode >$T/ref_$name.log 2>&1
timeout 600 $RUST slice -r $DEF "${G[@]}" -l $model "${M[@]}" -o $T/rust_$name.gcode >$T/rust_$name.log 2>&1
rc=$?
if cmp -s $T/ref_$name.gcode $T/rust_$name.gcode; then
  echo "$name: IDENTICAL ($(wc -l < $T/ref_$name.gcode) lines, $(md5sum < $T/ref_$name.gcode | cut -c1-8))"
else
  n=$(diff $T/ref_$name.gcode $T/rust_$name.gcode | grep -c '^[<>]')
  echo "$name: DIFFERENT (rc=$rc, $n differing lines; ref $(wc -l < $T/ref_$name.gcode) rust $(wc -l < $T/rust_$name.gcode 2>/dev/null))"
  diff $T/ref_$name.gcode $T/rust_$name.gcode | head -${CMP_SHOW:-8}
  grep -iE "panic|error" $T/rust_$name.log | head -3
fi
