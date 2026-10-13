#!/bin/bash
# Compare a dump stage between the instrumented C++ CuraEngine and the Rust port.
#
# usage: cmp_slice.sh <stage> <model.stl> <stop_after> [global args...] [-- mesh args...]
#   stage       record prefix to compare: walls | skin | overhang | support | adhesion | suppinfill
#   stop_after  CURA_STOP_AFTER value for the Rust engine (slice|walls|skin|support|adhesion|suppinfill)
#   global args are passed before -l (use "-e0 -s key=value" to override extruder-level settings)
#   mesh args are passed after -l (per-object settings).
#
# Environment:
#   REF_ENGINE  path to the instrumented reference engine (see reference/build_reference.sh)
#   RUST_ENGINE path to the Rust engine (default: <repo>/target/release/curaengine)
#   SETTINGS    resolved settings json (default: ender3_resolved.json next to this script)
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
REF_ENGINE=${REF_ENGINE:-curaengine_ref}
RUST_ENGINE=${RUST_ENGINE:-$REPO/target/release/curust-engine}
SETTINGS=${SETTINGS:-$HERE/ender3_resolved.json}
stage=$1; model=$2; stop=$3; shift 3
G=(); M=(); seen=0
for a in "$@"; do if [ "$a" == "--" ]; then seen=1; elif [ $seen == 0 ]; then G+=("$a"); else M+=("$a"); fi; done
T=${TMPDIR:-/tmp}/curust_cmp; mkdir -p "$T"
name=$(basename "$model" .stl)
CURA_DUMP=$T/ref_$name.txt "$REF_ENGINE" slice -r "$SETTINGS" "${G[@]}" -l "$model" "${M[@]}" -o "$T/ref_$name.gcode" >/dev/null 2>&1
CURA_STOP_AFTER=$stop CURA_DUMP=$T/rust_$name.txt "$RUST_ENGINE" slice -r "$SETTINGS" "${G[@]}" -l "$model" "${M[@]}" -o "$T/rust_$name.gcode" > "$T/rust_$name.log" 2>&1
awk -v s="^$stage" '$0 ~ /^[a-z_]+ mesh=/ {p = ($0 ~ s)} p' "$T/ref_$name.txt" > "$T/ref_${name}_stage.txt"
awk -v s="^$stage" '$0 ~ /^[a-z_]+ mesh=/ {p = ($0 ~ s)} p' "$T/rust_$name.txt" > "$T/rust_${name}_stage.txt"
h=$(md5sum < "$T/ref_${name}_stage.txt" | cut -c1-8)
if cmp -s "$T/ref_${name}_stage.txt" "$T/rust_${name}_stage.txt"; then
  echo "$name: IDENTICAL ($(grep -c "^$stage" "$T/ref_${name}_stage.txt") records, $h)"
else
  echo "$name: DIFFERENT"; diff "$T/ref_${name}_stage.txt" "$T/rust_${name}_stage.txt" | head -5; tail -3 "$T/rust_$name.log"
fi
