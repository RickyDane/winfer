#!/usr/bin/env bash
# Builds the native GPUI build for Windows in target/bundle:
#   winfer-native_<version>_windows-<arch>.exe         the portable .exe, also the updater bundle installed apps
#                                            swap in for themselves (updates.rs)
#   winfer-native_<version>_windows-<arch>-setup.exe the installer (Inno Setup, per user), when ISCC is found
# Run it from Git Bash.
# Usage: native/scripts/bundle-windows.sh [x86_64-pc-windows-msvc|aarch64-pc-windows-msvc]
set -euo pipefail
cd "$(dirname "$0")/.."
TARGET="${1:-$(rustc -vV | sed -n 's/host: //p')}"
VERSION="$(sed -n 's/^version = "\([^"]*\)".*/\1/p' Cargo.toml | head -1)"
ARCH="${TARGET%%-*}"
OUT="target/bundle"
EXE="$OUT/winfer-native_${VERSION}_windows-$ARCH.exe"
SETUP="winfer-native_${VERSION}_windows-$ARCH-setup"
cargo build --release -p winfer-app --bin winfer-native --target "$TARGET"
mkdir -p "$OUT"
cp "target/$TARGET/release/winfer-native.exe" "$EXE"
echo "$EXE"

ISCC="$(command -v iscc || command -v ISCC || true)"
for dir in "/c/Program Files (x86)/Inno Setup 6" "/c/Program Files/Inno Setup 6" "$LOCALAPPDATA/Programs/Inno Setup 6"; do
  [ -n "$ISCC" ] || { [ -x "$dir/ISCC.exe" ] && ISCC="$dir/ISCC.exe"; }
done
if [ -z "$ISCC" ]; then
  echo "Inno Setup (ISCC) not found: skipping the installer (choco install innosetup)" >&2
  exit 0
fi
case "$ARCH" in aarch64) INNO_ARCH=arm64 ;; *) INNO_ARCH=x64compatible ;; esac
winpath() { if command -v cygpath >/dev/null; then cygpath -w "$1"; else echo "$1"; fi; }
# MSYS_NO_PATHCONV: Git Bash would turn the /D… switches into paths.
MSYS_NO_PATHCONV=1 "$ISCC" /Q "/DAppVersion=${VERSION%%-*}" "/DSourceExe=$(winpath "$PWD/$EXE")" "/DArch=$INNO_ARCH" "/DOutName=$SETUP" "$(winpath "$PWD/scripts/winfer.iss")"
echo "$OUT/$SETUP.exe"
