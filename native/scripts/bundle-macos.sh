#!/usr/bin/env bash
# Builds winfer.app (the native GPUI build) and a .dmg next to it.
# Usage: native/scripts/bundle-macos.sh [aarch64-apple-darwin|x86_64-apple-darwin]
set -euo pipefail
cd "$(dirname "$0")/.."
TARGET="${1:-$(rustc -vV | sed -n 's/host: //p')}"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
OUT="target/bundle"
APP="$OUT/winfer.app"
cargo build --release -p winfer-app --bin winfer-native --target "$TARGET"
rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/$TARGET/release/winfer-native" "$APP/Contents/MacOS/winfer"
cp ../src-tauri/icons/icon.icns "$APP/Contents/Resources/icon.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>winfer</string>
  <key>CFBundleDisplayName</key><string>winfer</string>
  <key>CFBundleIdentifier</key><string>dev.winfer.native</string>
  <key>CFBundleExecutable</key><string>winfer</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>winfer uses the microphone for voice input and live voice.</string>
</dict></plist>
PLIST
# Ad-hoc signature (an unsigned arm64 binary won't run); set WINFER_SIGN_IDENTITY for a real one.
codesign --force --deep --sign "${WINFER_SIGN_IDENTITY:--}" "$APP"
ARCH="${TARGET%%-*}"
DMG="$OUT/winfer-macos-$ARCH.dmg"
rm -f "$DMG"
hdiutil create -volname winfer -srcfolder "$APP" -ov -format UDZO "$DMG" >/dev/null
echo "$APP"
echo "$DMG"
