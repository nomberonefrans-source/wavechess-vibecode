@echo off
rem WaveChess - one-click build of wavechess.exe (Windows)
cd /d "%~dp0"
where cargo >nul 2>nul
if errorlevel 1 (
  echo.
  echo Rust is not installed.
  echo 1. Download and run rustup-init.exe from https://rustup.rs
  echo 2. Accept the defaults, then CLOSE this window and run build_windows.bat again.
  echo.
  pause
  exit /b 1
)
echo Building release version, please wait...
cargo build --release
if errorlevel 1 (
  echo.
  echo BUILD FAILED.
  echo If the error says: linker link.exe not found
  echo   install "Build Tools for Visual Studio" with the workload
  echo   "Desktop development with C++":
  echo   https://visualstudio.microsoft.com/visual-cpp-build-tools/
  echo   then run this file again. Otherwise send the error text to the author.
  pause
  exit /b 1
)
if not exist dist mkdir dist
copy /y target\release\wavechess.exe dist\wavechess.exe >nul
echo.
echo Done: dist\wavechess.exe
echo Running self-test...
dist\wavechess.exe selftest
if errorlevel 1 (
  echo SELF-TEST FAILED.
  pause
  exit /b 1
)
echo.
echo Start the game with play.bat
pause
