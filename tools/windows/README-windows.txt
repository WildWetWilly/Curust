Curust - a 3D printing slicer: a Rust port of UltiMaker Cura 5.12
====================================================================

Files
-----
curust.exe            The application. Double-click it, or drop STL / 3MF / OBJ files onto it.
curust-cli.exe        The same program for the command line (run "curust-cli help").
curust-engine.exe     A drop-in for CuraEngine's command line ("curust-engine slice -j ...").
curust-resources.zip  Cura 5.12's printer definitions, materials and profiles. Curust unpacks it
                      to %APPDATA%\Curust\resources the first time it starts. If UltiMaker Cura
                      is installed, Curust can use Cura's own files instead.

Your printers, profiles, materials and settings are stored in %APPDATA%\Curust.

The window uses Microsoft Edge WebView2, which is part of Windows 10 and 11. Without it,
Curust opens in your web browser instead (the interface is served on 127.0.0.1 only).

Command line examples
---------------------
  curust-cli list-machines
  curust-cli slice-models --machine creality_ender3 --quality normal ^
      --set infill_sparse_density=30 -o part.gcode part.stl
  curust-cli scripts                         (post-processing scripts)
  curust-cli market-search --type material petg
  curust-cli usb-ports
  curust-cli usb-print --port COM3 part.gcode

License: AGPL-3.0-or-later (see LICENSE.txt). Cura's resource files are LGPL-3.0 / CC0.
