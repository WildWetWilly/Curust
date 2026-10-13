#!/bin/bash
# The official Linux CuraEngine 5.12 from the extracted AppImage, single-threaded (-m1) so its
# output can be compared run to run (see NOTES.md on thread-dependent output).
: "${CURUST_WORK:=${TMPDIR:-/tmp}/curust-work}"
: "${CURA_ROOT:=$CURUST_WORK/cura}"
exec env LD_LIBRARY_PATH="$CURA_ROOT" /lib64/ld-linux-x86-64.so.2 "$CURA_ROOT/CuraEngine" "$@" -m1
