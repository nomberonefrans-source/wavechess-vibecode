@echo off
chcp 65001 >nul
cd /d "%~dp0"
if exist dist\wavechess.exe (
  dist\wavechess.exe play
) else if exist wavechess.exe (
  wavechess.exe play
) else (
  echo wavechess.exe not found. Run build_windows.bat first.
  pause
)
