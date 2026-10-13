# Tools

Scripts for building releases and for checking Curust against Cura. None of this is needed to
build or use Curust.

## Paths

The comparison scripts read their locations from environment variables:

| Variable | Default | What it is |
|---|---|---|
| `CURUST_WORK` | `$TMPDIR/curust-work` | Scratch folder for builds, reference engines and results |
| `CURA_ROOT` | `$CURUST_WORK/cura` | UltiMaker Cura 5.12's Linux AppImage, extracted (`./UltiMaker-Cura-5.12.0-linux-X64.AppImage --appimage-extract`, then point this at `squashfs-root`) |
| `CURA_PYTHONHOME` | (none) | A CPython 3.12 installation, used as `PYTHONHOME` for Cura's bundled interpreter (`curapy`) |
| `REF_ENGINE` | `$CURUST_WORK/refbuild/curaengine_ref` | CuraEngine 5.12 built from source (`difftest/reference/build_reference.sh`) |
| `OFFICIAL_ENGINE`, `ORACLE_ENGINE` | `oracle/engine_m1.sh` | The official engine to compare with. `oracle/engine_win.sh` runs the Windows `CuraEngine.exe` under Wine instead |

## Releases

- `windows/cross_build.sh` builds the Windows release on Linux: `curust.exe`, `curust-cli.exe`,
  `curust-engine.exe`, `curust-standalone.exe` (Cura's resources built in) and
  `curust-installer.exe`, all in `dist/windows`. It downloads llvm-mingw and the Rust standard
  library sources on first use.
- `make_resources_zip.py` packs a Cura `resources` folder into `curust-resources.zip`, which the
  installer and the standalone exe carry.

## Comparing with Cura (`oracle/`)

The "oracle" is the real Cura 5.12 running headless: its Python front end loads a scene described
in a JSON job (printer, nozzle, material, quality, intent, settings, per-model settings, model
positions), slices it through its real backend and saves the G-code with its real writer.

Setting it up:

1. Extract the AppImage (see `CURA_ROOT` above).
2. Cura's Python code is frozen inside the AppImage. Extract the PyInstaller archive (for example
   with pyinstxtractor) into `$CURUST_WORK/pyx/out`, then run `oracle/merge.py` to build the
   module tree `boot.py` imports from.
3. Set `CURA_PYTHONHOME`.

Then:

| Script | What it does |
|---|---|
| `oracle/gcode_compare.py JOBS.json` | Slices each job in Cura and in Curust and compares the two G-code files. Job files: `gcode_jobs.json` (16 full scenes), `profile_jobs.json`, `material_jobs.json`, `blocker_jobs.json`, `pp_jobs.json` (post-processing) |
| `oracle/make_jobs.py`, `oracle/compare.py` | The settings comparison: every printer definition with its defaults, checked against `cargo run --example oracle_compare` |
| `oracle/engine_check.py`, `oracle/engine_rerun.py` | Run the official engine several times on the same input to show where Cura's own output changes from run to run |
| `oracle/profile_io_check.py` | `.curaprofile` export and import, including profiles from older Cura versions |
| `oracle/image_check.py`, `oracle/image_ref.py` | Image (lithophane) import against Qt's decoders and Cura's ImageReader |

## Engine tests (`difftest/`)

- `gcode_matrix.py` slices the cases in `gcode_cases.txt` with the reference engine and with
  `curust-engine` and compares them. `REF_REPEAT=2` runs the reference twice per case to spot
  cases where it isn't repeatable.
- `reference/` has the script and patch that build CuraEngine 5.12 from source as the reference.
- The `*_driver.cpp` files, `fuzz_clipper.py`, `voronoi/` and `wagyu/` fuzz Curust's ports of
  Clipper, wagyu, Boost.Polygon Voronoi, the libstdc++ containers and libpng against the C++
  originals.

## Interface and printing

- `gui_test/*.py` are Playwright tests of the interface. Start Curust with
  `curust gui --port 8790 --no-browser --resources DIR`, then run a test with the URL, a model
  and a folder for screenshots.
- `printsim/fake_marlin.py` is a simulated Marlin printer on a pseudo-terminal (it forces
  resends now and then). `printsim/fake_net.py` serves fake OctoPrint, Moonraker, PrusaLink and
  Duet APIs.
