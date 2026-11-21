#!/usr/bin/env bash
# Builds winfer.app (the native GPUI build), the updater bundle (.app.tar.gz) and a .dmg, all in
# target/bundle.
# Usage: native/scripts/bundle-macos.sh [aarch64-apple-darwin|x86_64-apple-darwin]
#   WINFER_SIGN_IDENTITY  a "Developer ID Application: …" identity in the keychain: signs with the
#                         hardened runtime (default: ad-hoc, "-")
#   WINFER_NOTARIZE=1     notarizes and staples the app, with either an App Store Connect API key
#                         (APPLE_API_KEY, APPLE_API_ISSUER, APPLE_API_KEY_PATH) or an Apple ID
#                         (APPLE_ID, APPLE_PASSWORD, APPLE_TEAM_ID)
set -euo pipefail
cd "$(dirname "$0")/.."
TARGET="${1:-$(rustc -vV | sed -n 's/host: //p')}"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
IDENTITY="${WINFER_SIGN_IDENTITY:--}"
ARCH="${TARGET%%-*}"
OUT="target/bundle"
APP="$OUT/winfer.app"
NAME="winfer-native-macos-$ARCH"
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

if [ "$IDENTITY" = "-" ]; then
  # Ad-hoc (an unsigned arm64 binary won't run).
  codesign --force --deep --sign - "$APP"
else
  # Developer ID: the hardened runtime and a secure timestamp, which notarization requires; the
  # Tauri app's entitlements keep the microphone working under the hardened runtime.
  codesign --force --options runtime --timestamp --entitlements ../src-tauri/Entitlements.plist --sign "$IDENTITY" "$APP"
fi

if [ "${WINFER_NOTARIZE:-}" = 1 ]; then
  if [ -n "${APPLE_API_KEY:-}" ]; then
    auth=(--key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER")
  else
    auth=(--apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID")
  fi
  zip="$OUT/$NAME-notarize.zip"
  ditto -c -k --keepParent "$APP" "$zip"
  result=$(xcrun notarytool submit "$zip" "${auth[@]}" --wait --output-format json) || true
  rm -f "$zip"
  echo "$result"
  if ! grep -Eq '"status" *: *"Accepted"' <<<"$result"; then
    id=$(sed -n 's/.*"id" *: *"\([^"]*\)".*/\1/p' <<<"$result")
    [ -n "$id" ] && xcrun notarytool log "$id" "${auth[@]}" || true
    echo "Notarization failed" >&2
    exit 1
  fi
  # The stapler talks to Apple's ticket servers: retry a dropped connection.
  for attempt in 1 2 3 4 5; do
    xcrun stapler staple "$APP" && break
    [ "$attempt" = 5 ] && exit 1
    sleep $((attempt * 15))
  done
fi

# The updater bundle: winfer.app in a .tar.gz (no AppleDouble files), which installed apps unpack in
# place of themselves (updates.rs).
COPYFILE_DISABLE=1 tar -czf "$OUT/$NAME.app.tar.gz" -C "$OUT" winfer.app

# The .dmg: the app beside a link to /Applications to drag it onto.
DMG="$OUT/$NAME.dmg"
STAGE="$OUT/dmg-$ARCH"
rm -rf "$STAGE" "$DMG" && mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname winfer -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"
echo "$APP"
echo "$OUT/$NAME.app.tar.gz"
echo "$DMG"
