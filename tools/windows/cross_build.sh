#!/bin/bash
# Cross-compiles the Windows release (x86_64-pc-windows-gnullvm) on Linux without any Windows
# SDK: llvm-mingw provides clang, lld and the MinGW-w64 UCRT libraries, and the Rust standard
# library is built from source for the target (-Zbuild-std) at the exact commit of the installed
# rustc. The executables only import DLLs that ship with Windows 10/11.
#
# usage: tools/windows/cross_build.sh [--no-portable] [--no-installer]
# result in dist/windows:
#   curust.exe, curust-cli.exe, curust-engine.exe   the programs (need curust-resources.zip or Cura)
#   curust-standalone.exe                           one file: the application with Cura's resources built in
#   curust-installer.exe                            the installer (all users, Start menu, "Open with", uninstaller)
#   curust-<version>-windows-x64.zip                the programs + resources as a zip
# env:   CURUST_WIN_WORK (default /tmp/curust-win), LLVM_MINGW (an extracted llvm-mingw release),
#        CURUST_RESOURCES_ZIP (default dist/windows/curust-resources.zip, see tools/make_resources_zip.py)
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
WORK=${CURUST_WIN_WORK:-/tmp/curust-win}
export CURUST_WIN_WORK=$WORK
mkdir -p "$WORK"
LLVM_MINGW=${LLVM_MINGW:-$WORK/llvm-mingw-20241217-ucrt-ubuntu-20.04-x86_64}
if [ ! -x "$LLVM_MINGW/bin/clang" ]; then
  curl -sSL -o "$WORK/llvm-mingw.tar.xz" https://github.com/mstorsjo/llvm-mingw/releases/download/20241217/llvm-mingw-20241217-ucrt-ubuntu-20.04-x86_64.tar.xz
  tar xf "$WORK/llvm-mingw.tar.xz" -C "$WORK"
fi
# Standard library source at the installed compiler's commit (plus its backtrace submodule).
COMMIT=$(rustc -vV | sed -n 's/^commit-hash: //p')
if [ ! -f "$WORK/rustsrc/library/std/Cargo.toml" ]; then
  rm -rf "$WORK/rustsrc" && mkdir -p "$WORK/rustsrc" && cd "$WORK/rustsrc"
  git init -q && git remote add origin https://github.com/rust-lang/rust
  git config core.sparseCheckout true && echo "library/" > .git/info/sparse-checkout
  git fetch -q --depth 1 --filter=blob:none origin "$COMMIT" && git checkout -q FETCH_HEAD
  BT=$(git ls-tree HEAD library/backtrace | awk '{print $3}')
  rmdir library/backtrace 2>/dev/null || true
  mkdir -p library/backtrace && cd library/backtrace
  git init -q && git remote add origin https://github.com/rust-lang/backtrace-rs.git
  git fetch -q --depth 1 origin "$BT" && git checkout -q FETCH_HEAD
fi
SRC_LINK="$(rustc --print sysroot)/lib/rustlib/src/rust"
mkdir -p "$SRC_LINK" && ln -sfn "$WORK/rustsrc/library" "$SRC_LINK/library"
# The standard library's crates.io dependencies, from their git repositories.
python3 "$ROOT/tools/windows/vendor_std_deps.py" > /dev/null
# The executables keep source paths for panic messages; map the build machine's folders to
# neutral names so no local paths end up in the release files.
SYSROOT=$(rustc --print sysroot)
CARGO_HOME_DIR=${CARGO_HOME:-$HOME/.cargo}
export PATH="$LLVM_MINGW/bin:$PATH" RUSTC_BOOTSTRAP=1 RUSTFLAGS="-C target-feature=+crt-static --remap-path-prefix=$ROOT=curust --remap-path-prefix=$WORK/rustsrc=rust --remap-path-prefix=$WORK=build --remap-path-prefix=$SYSROOT=rust --remap-path-prefix=$CARGO_HOME_DIR=cargo"
cd "$ROOT"
T=x86_64-pc-windows-gnullvm
OUT=target/$T/release
xbuild() {
  cargo build --release --target $T \
    -Zbuild-std=std,panic_abort,panic_unwind -Zbuild-std-features=panic-unwind,backtrace-trace-only \
    --config 'source.crates-io.replace-with="vend"' --config "source.vend.directory=\"$WORK/vendor\"" \
    --config "target.$T.linker=\"$LLVM_MINGW/bin/x86_64-w64-mingw32-clang\"" "$@"
}
strip_to() { "$LLVM_MINGW/bin/llvm-strip" --strip-debug -o "$2" "$1"; }
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
RES_ZIP=${CURUST_RESOURCES_ZIP:-$ROOT/dist/windows/curust-resources.zip}
mkdir -p dist/windows

# 1. The portable application: Cura's resources compiled in (extracted on first start).
if [[ " $* " != *" --no-portable "* ]]; then
  [ -f "$RES_ZIP" ] || { echo "missing $RES_ZIP (make it with tools/make_resources_zip.py)"; exit 1; }
  CURUST_EMBED_RESOURCES="$RES_ZIP" xbuild -p curust --bin curust
  strip_to "$OUT/curust.exe" dist/windows/curust-standalone.exe
fi

# 2. The regular programs (they use curust-resources.zip next to them, or an installed Cura).
CURUST_EMBED_RESOURCES= xbuild -p curust -p curust-engine-cli
for e in curust curust-cli curust-engine; do
  strip_to "$OUT/$e.exe" "dist/windows/$e.exe"
done

# 3. The zip and the installer, both with the same files.
STAGE="$WORK/stage"
rm -rf "$STAGE" && mkdir -p "$STAGE"
cp dist/windows/curust.exe dist/windows/curust-cli.exe dist/windows/curust-engine.exe "$STAGE/"
[ -f "$RES_ZIP" ] && cp "$RES_ZIP" "$STAGE/curust-resources.zip"
sed 's/$/\r/' LICENSE > "$STAGE/LICENSE.txt"
sed 's/$/\r/' tools/windows/README-windows.txt > "$STAGE/README.txt"
python3 - "$STAGE" "dist/windows/curust-$VERSION-windows-x64.zip" <<'EOF'
import os, sys, zipfile
src, out = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    for f in sorted(os.listdir(src)):
        # Already-compressed archives are stored as they are.
        z.write(os.path.join(src, f), "Curust/" + f, compress_type=zipfile.ZIP_STORED if f.endswith(".zip") else zipfile.ZIP_DEFLATED)
with zipfile.ZipFile(os.path.join(src, "..", "payload.zip"), "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    for f in sorted(os.listdir(src)):
        z.write(os.path.join(src, f), f, compress_type=zipfile.ZIP_STORED if f.endswith(".zip") else zipfile.ZIP_DEFLATED)
EOF
if [[ " $* " != *" --no-installer "* ]]; then
  CURUST_PAYLOAD="$WORK/payload.zip" xbuild -p curust-installer
  strip_to "$OUT/curust-installer.exe" dist/windows/curust-installer.exe
fi
ls -la dist/windows
