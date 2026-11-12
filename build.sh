#!/bin/sh
# WaveChess: сборка под Linux/macOS
set -e
cd "$(dirname "$0")"
cargo build --release
mkdir -p dist
cp target/release/wavechess dist/wavechess
./dist/wavechess selftest
echo "Готово: dist/wavechess  (запуск: ./dist/wavechess)"
