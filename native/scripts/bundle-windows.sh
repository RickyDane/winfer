#!/usr/bin/env bash
# Builds the native GPUI build for Windows as target/bundle/winfer-native-windows-<arch>.exe: a
# portable .exe, which is also the updater bundle installed apps swap in for themselves (updates.rs).
# Run it from Git Bash.
# Usage: native/scripts/bundle-windows.sh [x86_64-pc-windows-msvc|aarch64-pc-windows-msvc]
set -euo pipefail
cd "$(dirname "$0")/.."
TARGET="${1:-$(rustc -vV | sed -n 's/host: //p')}"
OUT="target/bundle"
EXE="$OUT/winfer-native-windows-${TARGET%%-*}.exe"
cargo build --release -p winfer-app --bin winfer-native --target "$TARGET"
mkdir -p "$OUT"
cp "target/$TARGET/release/winfer-native.exe" "$EXE"
echo "$EXE"
