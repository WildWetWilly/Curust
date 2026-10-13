<p align="center"><img src="crates/app/windows/curust-256.png" width="128" alt="Curust logo"></p>

# Curust

Curust is a port of UltiMaker Cura 5.12 to Rust. Both halves of Cura were ported: CuraEngine,
the C++ program that turns a model into G-code, and the Python front end that loads printers,
materials and profiles, works out the settings and talks to the engine. Load the same model with
the same printer and settings in Cura and in Curust, and you should get the same G-code file,
byte for byte. That was checked against the real Cura binaries, not judged by eye. The method
and results are further down.

It's about 106,000 lines of Rust with no outside crates. Clipper, mapbox wagyu, the Boost.Polygon
Voronoi builder, a small Python 3 interpreter (for Cura's post-processing scripts), PNG/JPEG
decoding, zip and the HTTP server are all part of the code. Licensed AGPL-3.0, like CuraEngine.

## Download

Windows 10 and 11, 64-bit. Get the files from the
[Releases page](https://github.com/WildWetWilly/Curust/releases).

| File | What it is |
|---|---|
| `curust-installer.exe` | Normal installer. Asks for administrator rights, installs to `Program Files\Curust` for all users, adds a Start menu shortcut (desktop shortcut optional), adds Curust to "Open with" for STL, 3MF and OBJ files, and puts an uninstaller in *Settings > Apps*. `curust-installer.exe /S` installs silently. |
| `curust-standalone.exe` | One file, nothing to install. Cura's printer and material files are built in. Handy on a USB stick or if you just want to try it. |

Both include the printer definitions, materials and profiles from Cura 5.12. If you have UltiMaker
Cura installed as well, Curust also picks up the materials and Marketplace packages you added
there. Your own printers, profiles and settings are kept in `%APPDATA%\Curust` either way.

The executables aren't code-signed, so Windows SmartScreen will probably warn you the first time
("Windows protected your PC"). Click *More info*, then *Run anyway*. The installer's permission
prompt will also say "Unknown publisher" for the same reason. If you'd rather not trust a
binary from the internet, build it yourself (see below). It only takes a Rust toolchain.

Linux works too, but there's no packaged build yet. Build from source and run `curust`, and the
interface opens in your browser. macOS should build but hasn't been tested.

## Will my prints come out the same as with Cura?

In every comparison that could be made, yes. The G-code was identical. Where it wasn't, Cura
itself gave a different file each time it sliced the same thing (see
[below](#where-identical-stops-being-possible)), so there was no single "right" file to match.
In those cases the differences are things like the order in which a few paths get printed. They
change from one Cura run to the next as well, and they don't change how the part comes out.

That said, this is a 1.0 of a big program, and the tests can't cover every combination of 600+
settings. For your first prints, look at the layer view before you print, the same way you would
with any new slicer. If something looks different from Cura, please open an issue and attach the
scene (menu > *Export scene as 3MF*) so it can be reproduced.

## What's included

**Slicing.** All of CuraEngine 5.12: Arachne variable-width walls, every infill pattern (gyroid,
lightning, cubic subdivision, cross 3D and the rest), normal and tree support with interfaces,
towers and conical support, skirt/brim/raft, seams including scarf seams, combing, coasting,
ironing, cooling, multi-extrusion with prime tower and ooze/draft shields, interlocking, texture
painting, one-at-a-time printing, spiralize, fuzzy skin, and every G-code flavor Cura supports.

**Printers, profiles and materials.** Cura's own resource files: 641 printer definitions with
their nozzles, materials, quality and intent profiles, the visibility presets, and every setting
formula. The formulas are evaluated by a Rust implementation of the Python subset Cura uses.
Custom profiles (create, update, rename, duplicate, delete), `.curaprofile` import and export
(including profiles stored in G-code files), custom materials (create, duplicate, edit, import
and export `.xml.fdm_material`), machine settings, per-model settings and the special mesh types
(infill, cutting, support and anti-overhang meshes).

**Post-processing.** Cura's post-processing plugin with all 20 of Cura's scripts: pause at
height, ChangeAtZ, filament change, search and replace, time-lapse, and so on. The scripts run
unmodified in pyrt, a Python 3 interpreter written for Curust (`crates/pyrt`). pyrt matches
CPython 3.12 where it shows up in G-code: float `repr`, `str.format` and `%` formatting, `sum`,
integers and the `re` module. It also covers the part of NumPy the Stretch script uses.
Scripts from the Marketplace, from Cura's scripts folder and your own `.py` files work too.

**Marketplace.** Searches UltiMaker's package server (same API and SDK version as Cura 5.12) and
installs `.curapackage` files. Materials, profiles, printer definitions and post-processing
scripts from packages all work. Plugins that are Python code for Cura's Qt interface will
install, but they don't do anything in Curust.

**Printing.**
- USB: Cura's serial protocol (line numbers, checksums, resends, `M105` temperature polling,
  `M115` firmware detection, baud-rate detection). The Monitor tab shows temperatures and
  progress and has pause/resume/abort, a jog panel and a console.
- Network: OctoPrint, Klipper/Moonraker (Mainsail, Fluidd), PrusaLink (API key or digest auth)
  and Duet/RepRapFirmware. Upload, start, status, temperatures, pause, resume, cancel and
  G-code commands.

**Model tools.** STL, OBJ and 3MF import, image/heightmap import with Cura's lithophane options,
move/scale/rotate (including lay flat by face), mirror, arrange, support blockers, and 3MF project
export in Cura's format.

**Interface.** Laid out like Cura: 3D build plate, recommended and custom settings, per-model
settings, layer preview with color schemes, Cura's G-code header and `;SETTING_3` footer. On
Windows it's a native window using WebView2 (part of Windows 10 and 11). Elsewhere it opens in
your browser, served on 127.0.0.1 only.

## How it was checked against Cura

The reference was the official UltiMaker Cura 5.12. The Linux AppImage was driven headless by the
scripts in `tools/oracle`, and the Windows `CuraEngine.exe` from the official MSI was run under
Wine with Microsoft's own runtime DLLs. For each test, Cura's real front end loaded the same
scene as Curust (same printer, nozzle, material, quality and intent, user settings, per-object
settings, model positions), sliced it with its real backend and saved it with its real
`GCodeWriter`. Then the two files were compared with `cmp`.

| Test | Result |
|---|---|
| Settings sent to the engine, for every printer Cura ships (default nozzle, material and profile) | 641 of 641 identical (global, per extruder, per object, `limit_to_extruder`) |
| Engine matrix: 302 model and setting combinations (every infill pattern, normal and tree support, adhesion types, seams, ironing, dual extrusion, prime tower, texture painting, spiralize, one-at-a-time and more) | 302 of 302 byte-identical to CuraEngine 5.12 built from source on Linux |
| The same matrix against the official Windows `CuraEngine.exe`, run twice per case | In 100 cases `CuraEngine.exe` produced the same file both times. Curust matched 99 of them, and the 100th (spiralize) also changed when Cura re-ran it. In the other 201 cases Cura's two runs already disagreed with each other |
| 16 full scenes saved by Cura on Linux (Ender-3, UM S5 dual with PVA, UM3, UM2+, Prusa MK3, Anycubic i3 Mega; brim, raft, tree support, per-model settings, intents, conditional start G-code) | 9 byte-identical. The other 7 come out differently from run to run in Cura itself |
| The same 16 scenes against the official Windows engine | 9 byte-identical (Curust's Windows build). The other 7 give Cura a different file on every run |
| Post-processing scripts | 32 of 32 script setups byte-identical, including the script settings Cura stores with the printer. Three more Stretch runs weren't counted because Cura slices those models differently from run to run, and thumbnails can't be compared because headless Cura can't render them |
| Custom profiles, `.curaprofile` export and import (including profiles from Cura 4.0 onward), custom materials, image/lithophane import | All byte-identical: 3 of 3, 13 of 13, 2 of 2, and 29 of 29 test images including the generated meshes |
| Support blockers | 2 of 3 byte-identical. The third uses tree support, where Cura's own output changes from run to run (Curust matches the reference engine there) |

The libraries underneath were fuzzed against the C++ originals: Clipper, wagyu (16,500 cases),
Boost.Polygon Voronoi (17,300 cases, including every multiprecision path), libstdc++'s red-black
tree, Boost's `concurrent_flat_map` and packed R-tree, and libpng's gamma handling (3,600 random
PNGs).

## Where "identical" stops being possible

Matching CuraEngine byte for byte meant tracking down everything its output depends on. Some of
it was surprising.

**Cura's output can depend on memory addresses.** A few CuraEngine containers are sorted or
hashed by pointer value: Arachne's junction connections (`unordered_set<edge_t*>`), parts of the
path order optimizers, tree support elements. Where things land in memory changes between runs,
and so does the G-code. Running the official Linux engine 8 times on the same input gave 2 or 3
different files for 6 of the 16 test scenes. The 7th only looked stable because of the TBB
allocator Cura ships. With plain glibc `malloc` swapped in, the same binary gave 3 different
results in 4 runs. On Windows, 4 runs gave 4 different files for each of those 7 scenes.
Microsoft's `std::hash` for pointers is FNV-1a of the address, so the order changes with every
heap layout. No program can match output that Cura doesn't repeat itself, so Curust uses a fixed
order (allocation order) in those places. `tools/oracle/engine_rerun.py` reproduces the
experiment.

**Cura for Windows and Cura for Linux write different G-code.** They disagree on 12 of the 16
test scenes. Seven of those are the scenes above, but five are perfectly repeatable on each
platform and still differ between them. Bisecting real slices turned up these causes:

- `std::sort` isn't stable, and how ties end up depends on the library. libstdc++ uses introsort
  with a 16-element insertion-sort cutoff. Microsoft's STL uses a "fat pivot" partition, Tukey's
  ninther and a 32-element cutoff. Clipper sorts with comparators that tie, so polygon offsets
  (brims, skirts) get different vertices. A plain cube with a brim on a UM2+ is enough to see it.
- `unordered_multimap::equal_range` returns equal keys newest first in libstdc++ and oldest
  first in Microsoft's STL. CuraEngine's `SparseGrid` uses it, which changes nearest-point
  tie-breaking (support zigzags, polyline stitching).
- `std::lerp` is `a + t*(b-a)` in libstdc++ and `fma(t, b-a, a)` in MSVC, so one rounding instead
  of two. It's used for fan speeds, scarf seams, speed gradients and support distances.
- `rand()` is glibc's additive feedback generator (`RAND_MAX` 2^31-1) on Linux and the UCRT's
  LCG (`RAND_MAX` 32767) on Windows. It picks random infill start points and drives fuzzy skin
  and lightning infill.
- Self-move-assignment of a `std::vector` empties it in libstdc++ and does nothing in MSVC. That
  happens in `Shape::removeSmallAreas`.
- `unordered_set<int>` hashes with the identity function and prime bucket counts in libstdc++, and
  with FNV-1a and power-of-two buckets in MSVC. Iteration order differs, which matters in tree
  support and interlocking.

Curust implements both. `crates/cppstd` has ports of the relevant parts of libstdc++ (GCC 13)
and Microsoft's STL (`sort`, `stable_sort` including comparators that aren't strict weak orders,
`list::sort`, the `unordered_set` hash table, `lerp`, `rand`). Each was checked against the
original code, compiled from source, on thousands of random inputs with lots of ties. Curust
behaves like the official Cura build for the platform it's running on, so the Windows build
matches Cura for Windows and the Linux build matches Cura for Linux. `CURUST_CXX_FLAVOR=msvc|gnu`
overrides that.

**The maths library matters too.** Microsoft's UCRT `sin`, `cos`, `acos`, `atan2`, `exp`, `log` and
`pow` differ from glibc's in the last bit for 6 to 14 percent of random inputs. The UCRT also picks
between two code paths depending on whether the CPU has FMA3, and those disagree on about 1
percent of `sin`/`cos` inputs. So Cura's Windows output can depend on which CPU it runs on. Curust's
Windows build calls the same UCRT functions as `CuraEngine.exe`, so it behaves the same way on the
same machine.

**Line endings.** Cura saves G-code through Python's text mode, so files saved on Windows have
`\r\n` line endings. Curust does the same.

Other things that had to be exact: `Point2LL * double` rounding with `llrint`, `float` versus
`double` arithmetic exactly where the C++ uses each, `Duration` clamping, `std::stable_sort`'s
buffered merge, libstdc++'s red-black tree shapes (the Voronoi beach line has a comparator that
depends on the sweep position), `std::hash<std::string>`, `printf("%g")`, CuraEngine's
`MMtoStream` quirks (`MMtoStream(-100)` prints `-.1`), Boost 1.86's `concurrent_flat_map`
iteration order and R-tree packing, and libpng's gamma conversion for painted textures.
[`NOTES.md`](NOTES.md) goes through all of it.

## Command line

The installer and the source build also include `curust-cli` (the same program, for scripts and
terminals) and `curust-engine` (CuraEngine's own command line).

```
curust-cli slice-models --machine creality_ender3 --quality normal \
    --set infill_sparse_density=30 --set support_enable=True \
    --add-script-json '["PauseAtHeight", {"pause_height": "12"}]' \
    -o part.gcode part.stl
curust-cli slice-models --printer "My Ender-3" -o plate.gcode plate.3mf   # a printer saved in the app
curust-cli list-machines                                                 # all 641 printers
curust-cli scripts                                                       # post-processing scripts
curust-cli market-search --type material petg                            # search the Marketplace
curust-cli install-package BASF_Ultrafuse_316L.curapackage
curust-cli usb-ports
curust-cli usb-print --port COM3 part.gcode
curust-cli net-print --kind moonraker --url http://voron.local part.gcode
curust-engine slice -j fdmprinter.def.json -s layer_height=0.2 -l model.stl -o out.gcode
curust-cli help
```

## Building from source

You need a recent stable Rust (<https://rustup.rs>). On Windows the default MSVC toolchain also
needs the Visual Studio Build Tools ("Desktop development with C++"), which `rustup-init.exe`
offers to install.

```
cargo build --release      # target/release/curust, curust-cli and curust-engine
cargo test --release --workspace
build.bat                  # the same thing on Windows, double-clickable
```

Curust needs Cura's resource files (printer definitions, materials, profiles). It finds them in
an installed UltiMaker Cura 5.x, in a `curust-resources.zip` next to the executable (make one with
`tools/make_resources_zip.py`), or wherever `--resources DIR` points.

The release files are cross-compiled on Linux by `tools/windows/cross_build.sh` (llvm-mingw, plus
a standard library built from source). It produces the installer, the standalone exe and the
plain executables in `dist/windows`.

## Project layout

| Crate | Lines | What's in it |
|---|---:|---|
| `engine` | 52,900 | The CuraEngine 5.12 port: slicing, Arachne, infill, support, tree support, path planning, G-code export. Translated function by function, keeping the names, control flow and evaluation order of the C++. |
| `app` | 13,500 | The application: interface server and web UI, scene, model import/export, slicing jobs, post-processing host, Marketplace, USB and network printing, saving and loading. |
| `pyrt` | 12,700 | The Python 3 interpreter for Cura's scripts: lexer, parser, tree-walking evaluator, and the builtins and modules the scripts use (`re`, `json`, `math`, `time`, `datetime`, `collections`, `functools`, `string`, `base64`, part of `numpy`). |
| `settings` | 9,300 | Cura's settings system: definition and instance containers, XML materials, the container registry, machine manager, setting stacks, the formula evaluator and StartSliceJob. |
| `clipper`, `wagyu`, `voronoi` | 12,800 | ClipperLib 6.4.2, mapbox wagyu 0.5 and Boost.Polygon Voronoi. |
| `cppstd` | 4,100 | The libstdc++ and Microsoft STL algorithms and containers whose behavior shows up in the output. |
| `installer` | 800 | The Windows installer and uninstaller (plain Win32, no dependencies). |

`tools/` has the comparison harness used for the results above, the GUI tests (Playwright), a
simulated Marlin printer and simulated OctoPrint, Moonraker, PrusaLink and Duet servers. See
[`tools/README.md`](tools/README.md).

## Contributing

Bug reports, fixes and new features are welcome. [`CONTRIBUTING.md`](CONTRIBUTING.md) explains how
the code is organized, how to test a change against Cura, and the one big rule: anything that
changes the G-code has to keep it identical to what Cura writes.

## Known limitations

- Cura plugins that add to Cura's Qt interface (Python code) can't run. The data inside them
  (materials, profiles, printer definitions, post-processing scripts) does work.
- UltiMaker Digital Factory (cloud printing) and UltiMaker's own network printing for the UM3 and
  S-line printers aren't implemented. Save to a file or print over USB instead.
- The `Create Thumbnail` script gets its image from Curust's own software renderer. It uses Cura's
  camera position, but the picture isn't pixel-identical to Cura's OpenGL snapshot.
- macOS hasn't been tested. Cura's macOS engine is built with clang and libc++, which would be a
  third C++ standard library to emulate.

## License

AGPL-3.0-or-later (see [`LICENSE`](LICENSE)), the same as CuraEngine. Cura's resource files and
post-processing scripts belong to UltiMaker and are under the licenses given in Cura's
repositories (LGPL-3.0, CC0 for the material profiles). The Microsoft STL algorithms in
`crates/cppstd/src/msvc.rs` are translated from microsoft/STL (Apache-2.0 WITH LLVM-exception).

Curust is an independent project. It isn't affiliated with or endorsed by UltiMaker. "Cura" and
"UltiMaker" are trademarks of UltiMaker.
