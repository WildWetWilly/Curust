# Curust — architecture and porting notes

## Architecture

Curust mirrors Cura's own split between a front end and a slicing engine process:

```
curust (GUI)                                         curust engine-job (child process)
 ├─ server.rs   HTTP on 127.0.0.1, JSON API  ──job──▶  communication/job.rs  (Arcus equivalent)
 │   └─ web/    index.html, app.js, viewer.js           └─ curust-engine  (CuraEngine 5.12 port)
 │              (WebGL2), shown in a native window    ◀─result── G-code body, layer paths,
 │              (winview.rs, WebView2) or a browser               time / material estimates,
 ├─ session.rs  printers, profiles, scene                         "PROGRESS x" lines on stdout
 ├─ slicing.rs  StartSliceJob checks, job, engine run, G-code assembly (Cura's GCodeWriter)
 └─ curust-settings  Cura's settings system
```

* **Settings** (`crates/settings`): parsers for `*.def.json`, `*.inst.cfg` and `*.xml.fdm_material`;
  the container registry (with Cura's query semantics and `uniqueName`); the container tree
  (machine → variant → material → quality → intent) and machine manager defaults; setting stacks
  (user, quality changes, intent, quality, material, variant, definition changes, definition);
  a Python-subset evaluator for setting formulas (`py/`: `extruderValue(s)`, `resolveOrValue`,
  `math`, list comprehensions, Python's int/float semantics incl. big-int overflow);
  `slice_message.rs` ports StartSliceJob (setting messages, `limit_to_extruder`, per-object
  settings, start/end G-code token expansion with `GcodeStartEndFormatter` semantics).
* **Front end** (`crates/app`): scene nodes with Cura's vertex pipeline (STL float32 `(x, z, -y)`,
  centring, `position.y = -min_y`, f32 transforms) so the engine receives the identical meshes;
  convex hulls / disallowed areas / build-volume checks (`hull.rs`, `buildvolume.rs`); arrange;
  STL / OBJ / 3MF import, 3MF export (`mesh_io.rs`, `zip.rs` with deflate); the job (all the
  messages Cura sends over Arcus) and the result (`communication/job.rs`); the final file like
  `CuraEngineBackend` + `PrintInformation` + `GCodeWriter` (header prefix, `{print_time}` /
  `{filament_*}` / `{jobname}` replacement, `;SETTING_3` footer with Cura's ConfigParser
  serialization and 69-character chunks).
* **Engine** (`crates/engine`): the CuraEngine 5.12 port, see below.
* **Window** (`crates/app/src/winview.rs`, Windows): a Win32 window hosting WebView2 through its
  COM interfaces directly (no WebView2Loader.dll: the runtime is located like the loader does,
  and `CreateWebViewEnvironmentWithOptionsInternal` of its `EmbeddedBrowserWebView.dll` is
  called). Elsewhere, or when WebView2 is missing, an Edge/Chrome `--app` window or the default
  browser is used.
* **Interface** (`crates/app/web`): plain HTML/CSS/JS embedded in the executable
  (`CURUST_WEB_DIR` serves them from disk during development). The server is single-threaded and
  std-only; slicing runs in the child process so the interface stays responsive and a slice can
  be cancelled by killing it. The program ends when its window closes.

* **Post-processing** (`crates/app/src/postproc.rs`, `postproc_shim.py`, `crates/pyrt`): Cura's
  PostProcessingPlugin. The scripts (`crates/app/postproc/scripts`, Cura's files) run unmodified
  in pyrt, Curust's Python 3 interpreter. `postproc_shim.py` provides the parts of Cura and
  Uranium they import (Application, containers and stacks with Uranium's string → type
  conversion and SettingFunction evaluation, Message, Logger, PrintInformation, Snapshot,
  QImage/QBuffer for thumbnails); the host side (`HostModule`) answers from the Curust session.
  Scripts are stored like Cura stores them (`post_processing_scripts` metadata of the global
  stack: ConfigParser sections, escaped, `str(getSettingValueByKey(k))` for every setting when
  the dialog closes) and run when the G-code is written, over Cura's `gcode_list` chunks
  (`SliceResult.gcode_chunks` records the engine's flush boundaries), with the `;POSTPROCESSED`
  footer lines. Thumbnails come from a software renderer (`snapshot.rs`) with Cura's camera.
* **Marketplace** (`marketplace.rs`): UltiMaker's package API with Cura 5.12's SDK version
  (`cura-packages/v1/cura/v8.12.0/packages`), downloads through the system `curl`, installs
  `.curapackage` files like Cura's PackageManager (`files/materials`, `qualities`, `variants`,
  `intent(s)`, `plugins`); resources and scripts inside installed plugins, and packages
  installed in UltiMaker Cura (newest `cura/<version>` data folder), are loaded.
* **Printing**: `serial.rs` (serial ports: Win32 COM API, termios through `stty` on Unix),
  `usbprint.rs` (Cura's USBPrinterOutputDevice protocol: `N<n> <line>*<xor>` with `M110`,
  one line per `ok`, `Resend`, `M105` every 3 s, `M115`, AutoDetectBaudJob's baud list),
  `netprint.rs` (OctoPrint, Moonraker, PrusaLink, Duet RRF; a small HTTP/1.1 client, `curl` for
  HTTPS and digest authentication). `tools/printsim` has a fake Marlin on a PTY (with forced
  resends) and fake OctoPrint / Moonraker / PrusaLink / Duet servers.
* **Windows packaging**: `crates/installer` (curust-installer.exe: Win32 UI, asks for
  administrator rights through its manifest and installs to `%ProgramFiles%\Curust` for all users,
  HKLM registry entries, shortcuts through PowerShell's WScript.Shell, removes a per-user 0.1
  installation, starts Curust unelevated through Explorer, and an uninstaller that removes only
  the files listed in `install-files.txt`); the standalone exe embeds `curust-resources.zip`
  (`CURUST_EMBED_RESOURCES` at build time, `resources.rs` extracts it once per archive).

Executables: `curust` (application, Windows GUI subsystem), `curust-cli` (same, console),
`curust-engine` (CuraEngine command line). All work runs on a thread with a 64 MiB stack
(Windows main threads only have 1 MiB).

Environment variables: `CURUST_RESOURCES` (Cura resources directory), `CURUST_CONFIG_DIR`,
`CURUST_BROWSER` (browser executable), `CURUST_USE_BROWSER` (no native window), `CURUST_ENGINE` (engine executable),
`CURUST_WEB_DIR`, `CURUST_DUMP_JOB` (write the engine job file), `CURUST_UTC_OFFSET_MINUTES`,
`CURA_FIXED_UUID`, `CURA_HW_CONCURRENCY`, `CURUST_BUILD_DATE` (compile time),
`CURUST_CXX_FLAVOR` (`gnu` / `msvc`: which C++ standard library to emulate, default: the
platform's official Cura build), `CURUST_TEXT_MODE` (`lf` / `crlf` line endings of written files,
default: the platform's), `CURUST_EMBED_RESOURCES` (build time: resources archive to embed),
`CURUST_SCRIPTS` (extra post-processing script folder), `CURUST_NO_CURA_SCRIPTS`,
`CURUST_NO_CURA_USER_DIR`, `CURUST_NO_USER_RESOURCES`, `CURUST_MARKETPLACE_URL`,
`CURUST_EXTRA_SERIAL` (extra serial port names, e.g. a PTY), `CURUST_DUMP_PP_META`,
`CURUST_DUMP_CHUNKS`.

## Verification against real Cura 5.12

`tools/oracle` runs the real Cura 5.12 front end headless (Python 3.12, Uranium + Cura modules
from the installation, offscreen Qt) and records what it sends to the engine:

* `boot.py` / `oracle_task.py`: creates printers, sets variants / materials / quality / intent,
  user and per-object settings, loads models at fixed positions, runs StartSliceJob, and
  optionally slices for real (`gcode_out`, CuraEngine with `-m1`) and saves the G-code through
  Cura's GCodeWriter.
* **Cura's container registry iterates Python sets**, so its default quality / material choices
  depend on `PYTHONHASHSEED` (seeds 0, 1 and 2 give different default qualities for some
  printers). The oracle patches the registry to iterate sorted, and Curust loads profiles in
  sorted container-id order, which reproduces that deterministic variant.
* `make_jobs.py` generates jobs (every machine definition with defaults; profile / setting
  variations); `compare.py` compares the recorded settings with
  `cargo run --example oracle_compare` (the Rust port on the same jobs).
  Result: **641/641 machine definitions identical** (all global, extruder and per-object settings,
  `limit_to_extruder`, container stacks).
* `gcode_compare.py gcode_jobs.json` slices complete scenes in both and compares the files:
  9/16 byte-identical (only `;SLICE_UUID` differs).
* `engine_check.py` explains the other 7: it re-runs the engine stage alone on Curust's job file
  with the official CuraEngine binary (8 times, changing only the length of the output-file path
  string), the deterministic reference build, and Curust. In all 7 Curust equals the reference,
  and the official binary produces 2–3 different G-code files depending on that string length:
  its output depends on heap addresses (pointer-keyed containers, see below), so it cannot be
  reproduced byte for byte by any other program — and Cura itself gives different G-code when its
  memory layout differs.
* `engine_rerun.py` makes it plainer: the *identical* command line, 8 runs with address-space
  layout randomisation: 6 of the 7 scenes give 2–3 different files (with ASLR off, each run gives
  the same file). The 7th (`ender_perobject`) only looks stable because the official binary links
  TBB's malloc proxy (`libtbbmalloc_proxy.so`), whose layout survives ASLR: with glibc's malloc
  preloaded (an `LD_PRELOAD` shim that calls glibc's `malloc` from `libc.so.6` itself, since the
  TBB proxy also exports `__libc_malloc`) the same binary gave 3 different files in 4 runs, and the
  same source compiled without patches, linked against TBB's proxy, gave yet another result.
* Post-processing: `gcode_compare.py` with `post_processing` jobs (`oracle_task.py` drives Cura's
  PostProcessingPlugin: `loadAllScripts`, `addScriptToList`, setting values, `writeScriptsToStack`,
  `execute`) compares the saved files and the stored `post_processing_scripts` metadata.

### The Windows build of Cura

UltiMaker's Windows `CuraEngine.exe` is built with MSVC, Microsoft's STL and the Universal CRT
(no TBB malloc proxy: the CRT heap). `tools/oracle/engine_win.sh` runs the official one (from
`UltiMaker-Cura-5.12.0-win64-X64.msi`) under Wine with the Microsoft runtime DLLs that ship with
Cura (`WINEDLLOVERRIDES=ucrtbase,msvcp140,…=n,b`), and `gcode_compare.py` with
`ORACLE_ENGINE=engine_win.sh` drives it from Cura's front end. Findings:

* It disagrees with the Linux build on 12 of the 16 scenes. 7 are the address-dependent ones
  (on Windows 4 runs gave 4 different files for each); 5 are deterministic on both platforms and
  still different. Bisecting them found the causes, all now emulated in `cppstd` (selected by
  `cppstd::flavor`):
  * `std::sort` ties (Clipper's local minima and intersection list): MSVC's introsort
    (`_Partition_by_pivot_unchecked` "fat" partition, Tukey's ninther over 40 elements,
    insertion sort up to 32) — a UM2+ cube with a brim is enough to see it;
  * `unordered_multimap::equal_range` order: newest first in libstdc++, insertion order in MSVC
    (`SparseGrid`, the inset order graph);
  * `std::lerp`: `fma(t, b - a, a)` in MSVC;
  * `rand()`: UCRT's LCG (`x = x * 214013 + 2531011`, `(x >> 16) & 0x7fff`);
  * self-move-assignment of a vector (`Shape::removeSmallAreas`): MSVC keeps the elements;
  * `std::unordered_set` (doubly linked list, power-of-two buckets growing ×8 below 512, insert at
    the end for a new bucket and before the bucket's first element otherwise, FNV-1a `std::hash`
    for integers and strings); `std::stable_sort` and `std::list::sort` (Microsoft's merge
    patterns, identical for strict weak orders, but CuraEngine has comparators that aren't).
  `cppstd/examples/msvc_*_check.rs` compare the sorts with Microsoft's code compiled from source
  (`tools/difftest`-style harnesses, 10,000 random tie-heavy cases incl. inconsistent comparators).
* The UCRT's `sin`/`cos`/`acos`/`atan2`/`exp`/`log`/`pow` differ from glibc's in the last bit for
  6–14 % of random arguments, and the UCRT picks FMA3 or SSE2 code at run time (`sin`/`cos`
  differ between the two paths for ~1 % of arguments, see `_set_FMA3_enable`). Rust on
  `x86_64-pc-windows-gnullvm` calls the UCRT's functions, so the Windows build of Curust gets the
  same results as `CuraEngine.exe` on the same machine. None of the 16 scenes changed between
  glibc and UCRT maths.
* Result with `CURUST_CXX_FLAVOR=msvc` (and the Windows build under Wine): **9/16 byte-identical**
  to the official Windows engine; the other 7 are the address-dependent scenes, where every run of
  `CuraEngine.exe` produced a different file. `gcode_matrix.py` with `REF_ENGINE=engine_win.sh`
  and `REF_REPEAT=2` runs the engine matrix the same way.
* Files: CuraEngine's `std::ofstream` and Cura's `open(..., "wt")` write `\r\n` line endings on
  Windows; Curust does the same (`utils/text_mode.rs`).

## Engine porting approach

The port translates CuraEngine 5.12 function by function, keeping names, control flow and
evaluation order. Byte-identical output is the goal, so C++ numeric and library semantics are
reproduced exactly:

* `Point2LL * double` rounds with `llrint` (double precision), `* float` in single precision,
  `* integer` is exact integer arithmetic (`geometry::point2ll::scale_*`).
* `Ratio`, `Velocity`, `AngleRadians`, ... are plain `f64` arithmetic (`NumericFacade` has no
  integral overloads); conversions back to `coord_t` truncate.
* `Duration` clamps negatives in its constructor (so `duration += double` clamps the addend) but
  `+=` of two durations does not.
* libstdc++ algorithms whose results depend on the implementation (introsort, heap layout,
  `stable_sort` merge order, `std::map` tree shape, `rand()`) are emulated in `crates/cppstd`.
* Output formatting: `PrecisionedDouble` / `MMtoStream` (`utils::string`) are reproduced exactly,
  including quirks (`MMtoStream(0)` prints `0.00`, `MMtoStream(-100)` prints `-.1`); default
  `ostream` (`%g`) and `std::fixed` formatting are emulated.
* Floating point formatting relies on correctly-rounded exact formatting, which Rust's `{:.N}`
  provides identically to glibc's `printf` (verified on 200k random values incl. ties).

## Deliberate deviations / known parity limits

* **Nondeterminism in CuraEngine**: several containers are keyed or ordered by heap address, so
  the C++ engine's output depends on the allocator. The reference build is patched to use a
  deterministic order instead, and the Rust port reproduces the patched behaviour:
  * `SkeletalTrapezoidation::connectJunctions` (`unordered_set` keyed by pointer): graph order.
  * `PathOrderOptimizer`, `PathOrderMonotonic`, `InsetOrderOptimizer` (pointer-keyed sets/maps).
  * `TreeSupport`: every `new TreeSupportElement` gets an allocation serial (`serial_`);
    `std::less<TreeSupportElement*>` is specialised to compare serials, so `std::set` /
    `std::map<TreeSupportElement*, ...>` iterate in allocation order, and the
    `unordered_map/unordered_set<TreeSupportElement*, ...>` containers are replaced by `std::map` /
    `std::set`. The Rust port keeps elements in an arena whose index is that serial.
* **Multithreading**: the reference is only deterministic with `-m1` (single thread; `parallel_for`
  then runs chunks in index order). Random seams, lightning infill and tree support depend on it.
  The Rust port always computes in that order.
* **Machine-dependent output**: `TreeSupport::mergeInfluenceAreas` splits the influence areas into
  `min(2^floor(ln(n/2)), std::thread::hardware_concurrency())` buckets and only merges within a
  bucket, so tree support differs between machines with different core counts. Both engines read
  `CURA_HW_CONCURRENCY` to override it (the port otherwise uses `available_parallelism()`); the
  test harness sets it to 2.
* `CURA_FIXED_UUID` (both engines) fixes the Griffin `;SLICE_UUID` for testing.
* `MeshMaterialSplitter`: the voxel grid and the marching-squares contours live in
  `boost::concurrent_flat_map/set`, and the propagation looks up the nearest painted voxel in a
  packed `boost::geometry::index::rtree`; their iteration / tie-breaking order decides the output.
  `cppstd::boost_foa` and `cppstd::boost_rtree` emulate Boost 1.86 exactly (verified against the
  real containers with random operation sequences). The parallel visits (`std::execution::par`)
  run serially in the reference (no TBB), in group order.
* Painted textures are decoded like libpng's simplified API does (natural format, palette images
  rejected, 8-bit output gamma-corrected to sRGB when a `gAMA`/`sRGB` chunk says otherwise, 16-bit
  output linear, little-endian and alpha-premultiplied); verified against libpng 1.6.43 on 3600
  random PNGs (all colour types / bit depths, tRNS, interlacing, gAMA/sRGB, tEXt/zTXt/iTXt).
  Not supported: 16-bit textures that carry gamma information (libpng would convert them to
  linear light with its 16-bit gamma tables); such textures are not loaded. iCCP profiles are
  ignored for the gamma decision.
* Settings that make the C++ engine hang or crash are not reproduced (e.g. `support_tree_angle=90`
  loops practically forever in `TreeModelVolumes::safeOffset`; interlocking with
  `interlocking_boundary_avoidance=0` indexes negative layers).
* Transcendental functions (`sin`, `cos`, `atan2`, `pow`, ...) come from the platform's libm, as
  in the C++ engine. The reference is glibc; other C libraries may differ in the last bit for rare
  inputs, which can (very rarely) change a coordinate in the output.
* Griffin/Cheetah headers contain the engine build date (`__DATE__` in C++); the port uses
  `CURUST_BUILD_DATE` (compile-time env) or a fixed default.

## C++ quirks reproduced on purpose

* `AreaSupport::precomputeCrossInfillTree` assigns the extruder settings through a `Settings&`
  bound to a support mesh's settings, overwriting that mesh's settings (cross/cross-3D support only).
* `SkirtBrim::getFirstLayerOutline` loops over `std::array{gapped, touching}` (copies), so the
  join/simplify in that loop has no effect.
* `Comb::getModelBoundary` computes `model_boundary_` but returns `boundary_outside_`.
* `LinePolygonsCrossings` / `Comb` penalty functions return `int`: the `coord_t` values are
  narrowed.
* Tree support:
  * `config.getRadius(parent)` in `generateBranchAreas` is called with a pointer, which converts
    through the implicit `TreeSupportElement(TreeSupportElement*)` constructor (distance to top + 1,
    result position unset), so the parent's ovalisation radius is the one of a hypothetical child.
  * `fast_relative_movement` is evaluated before `generateArea` updates `max_speed_sqd`, so it is
    always false.
  * `splitLines` starts both results with one empty line, so the xy-overrides-z lag loop never
    stops because its line list became empty.
  * `mergeInfluenceAreas` only ever merges within a bucket (the loop ends after the first round,
    because `buckets_area` is cleared, not merged).
  * `TreeModelVolumes` caches never overwrite (`unordered_map::insert`), and data vectors inserted
    in bulk add `(radius, -1)` / `-1` placeholder keys.
  * `setToModelContact` passes `use_min_xy_dist` as the `to_model` argument of `getAvoidance`.
  * The copy constructor of `TreeSupportElement` does not copy `additional_ovalization_targets_`.
* `Image::getPixelCoordinates` converts `float` to `size_t` the x86-64 way: UV coordinates below
  -1/width wrap around and get clamped to the last pixel, not the first.
* `MeshMaterialSplitter::sliceMesh` formats the xy offset with `std::to_string` (6 decimals).
* `CommandLine -r` keeps the file's sections in `std::unordered_map<std::string, ...>`s, so the
  extruders and the models listed in the file are processed in libstdc++ hash-table order
  (`cppstd::unordered` + `std::hash<std::string>` = `_Hash_bytes`, verified against libstdc++). An
  `extruder.N` section processed before lower-numbered ones creates the missing extruder trains
  with number N; model settings accumulate in the mesh group's settings from one model to the next.

## Verified stages (identical to reference)

Dump records compared byte-for-byte with the instrumented reference engine:

* `slice` — layer outlines.
* `walls` — Arachne toolpaths for 11 models × many settings (wall counts 1/6/8, distribution count,
  fill_outline_gaps, wall_0_inset, fluid motion off, alternate extra perimeter, transition angle,
  line widths, spiralize).
* `skin` — infill/skin areas, roofing/flooring, expansion, preshrink, ironing, top/bottom layers 0.
* `overhang`, `support` — normal support with buildplate/everywhere, interfaces on/off, roof/bottom
  heights & offsets, xy-overrides-z both ways, stair stepping, towers, conical support, support
  angle, minimum areas, support meshes (with/without drop down), anti-overhang meshes, infill meshes.
* `adhesion` — skirt (incl. minimal length), brim (inside/outside/everywhere, minimal length),
  brim + support (brim replaces support), none, raft outlines, draft shield (full/limited),
  ooze shield.
* `suppinfill` — gradual support infill, support infill sparse thickness, support walls.
* Libraries: clipper (fuzzed), wagyu (16.5k fuzz cases), voronoi (17.3k fuzz cases incl. all
  multiprecision paths), rb-tree shapes.

## Full G-code comparisons

`tools/difftest/gcode_matrix.py` + `gcode_cases.txt`: about 330 cases (11 models plus painted
variants, single and dual extrusion, per-mesh settings) compared byte for byte, all identical.
Texture painting: 23 cases (multi-material splitting with several resolutions / depths, painted
extruder ≠ mesh extruder, inactive extruder values, fully / not painted textures, RGB / RGBA / gray
/ gray-alpha textures, gamma-corrected texture, several painted meshes, support, prime tower;
painted seams with shortest / sharpest corner / random / back seams, surface mode, spiralize).
Tree support: 62 cases
(rest preference, xy/z priority, angles, diameters, reach limit, all support/roof patterns, walls,
fractional roofs, fake roofs, interface priorities, raft/brim, several meshes, dual extrusion with a
prime tower).

## Front-end parity notes

* Cura's vertex pipeline is reproduced in f32/f64 exactly (`scene.rs`), including the centring
  `center = min + (max - min) / 2` and the transform order, so the engine sees bit-identical
  vertices. Arrange (`scene::arrange`) is Curust's own (Cura uses pynest2d with random
  rotations); positions given explicitly are honoured exactly.
* Profile choice order follows the sorted registry (see above); duplicate model names get
  Cura's `name(1)` renaming; extruder stacks are named `"<extruder definition> #2"` like
  `uniqueName` produces in a fresh Cura.
* `{print_time}` uses the sum of the engine's per-feature f32 times in Cura's dict order with
  round-half-even; material lengths `round(amount / (π r²) / 1000, 2)`, weights from the
  material density, costs from the preferences.
* Not ported: UltiMaker Digital Factory (cloud) and the UM3/S-line network (cluster) output
  devices, Cura plugins that are Python code for Cura's Qt interface, and the SolidWorks reader.
