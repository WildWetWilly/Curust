@echo off
rem Builds Curust (release) with the Rust toolchain from https://rustup.rs
rem Result: target\release\curust.exe (the application), curust-cli.exe, curust-engine.exe
setlocal
cd /d "%~dp0"

where cargo >nul 2>nul
if errorlevel 1 (
  echo Rust is not installed.
  echo Install it from https://rustup.rs ^(rustup-init.exe, default options^) and run build.bat again.
  echo.
  pause
  exit /b 1
)

cargo build --release
if errorlevel 1 (
  echo.
  echo The build failed. With the default MSVC toolchain, the "Desktop development with C++"
  echo workload of the Visual Studio Build Tools must be installed ^(rustup-init offers it^).
  echo.
  pause
  exit /b 1
)

echo.
echo Built target\release\curust.exe - start it to open Curust.
echo.
pause
