# Contributing to Curust

Thanks for taking a look. Bug reports, fixes, new features and plain questions are all welcome.

## Reporting a bug

Open an issue and include:

- the Curust version (menu > *About Curust*, or `curust-cli --version`) and your OS
- the printer, nozzle, material and profile you used
- what you expected and what happened
- if it's about slicing: the scene saved as 3MF (menu > *Export scene as 3MF*) and, if you can,
  the G-code from Cura for the same models and settings. A G-code difference against Cura is
  the most useful bug report there is.

## The one rule for slicing changes

Curust's whole point is that it writes the same G-code as Cura 5.12. Any change to the engine
(`crates/engine`), the settings system (`crates/settings`) or the parts of the app that build the
engine job or the final file has to keep that true. A change that "improves" the output compared
to Cura is a fork of Cura's behavior, and it would need to be an opt-in setting.

That also means some odd-looking code is on purpose. If you see `f32` where `f64` seems better,
an `llrint` instead of `round`, a sort that looks like it should be stable, or a container that
emulates libstdc++ or Microsoft's STL, it's almost certainly there because CuraEngine's output
depends on it. `NOTES.md` explains most of these. When in doubt, ask in the issue first.

Changes to the interface, printing, the Marketplace, the installer and so on don't have this
constraint, beyond keeping the files Curust writes (profiles, materials, 3MF, settings stored with
the printer) compatible with Cura.

## Building and testing

```
cargo build --release
cargo test --release --workspace
```

Running the app needs Cura's resource files. Either have UltiMaker Cura 5.x installed, put a
`curust-resources.zip` next to the executable (`tools/make_resources_zip.py` makes one), or
start it with `--resources DIR` pointing at a Cura `resources` folder (the folder that contains
`definitions/fdmprinter.def.json`).

During interface work, `CURUST_WEB_DIR=crates/app/web` serves the HTML, CSS and JavaScript from
disk, so a browser reload picks up changes without rebuilding.

The interface tests in `tools/gui_test` use Playwright for Python:

```
target/release/curust gui --port 8790 --no-browser --resources DIR &
python3 tools/gui_test/smoke.py http://127.0.0.1:8790/ tests/models/cube20.stl /tmp/shots
```

## Checking slicing changes against Cura

The full comparison setup (headless Cura, the official engines, the reference engine built from
source) is described in [`tools/README.md`](tools/README.md). It's a bit of work to set up, so
for a small change it's fine to do the simplest version: slice a few models in Cura 5.12 and in
Curust with the same settings, save both files and compare them (`cmp`, or `fc /b` on Windows).
Use positions that are fixed rather than auto-arranged, and `--fixed-time` on the Curust side if
you use the command line.

If the files differ, `tools/difftest` has helpers to narrow it down to the first differing layer
and feature.

## Code style

- `cargo fmt` isn't enforced yet, but keep new code close to the style around it.
- The engine keeps CuraEngine's file layout, function names and order of operations as far as
  Rust allows, so a function can be compared side by side with the C++. Please keep it that way.
- No new third-party crates without a good reason. Curust builds with nothing but the standard
  library, and that keeps the Windows cross-build simple.

## Licensing

Curust is AGPL-3.0-or-later. By sending a pull request you agree that your contribution is
licensed the same way.
