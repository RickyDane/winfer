#!/usr/bin/env bash
# Runs the debug build as a real app bundle, so macOS privacy prompts (microphone, speech
# recognition for the composer's dictation) are asked for by it instead of the terminal or
# installed winfer that started it, which would get the process killed without their usage texts.
#
# Usage: native/scripts/dev-app-macos.sh   (WINFER_* variables set when calling it are passed on)
# It keeps its data apart from the installed app (WINFER_NATIVE_IDENTIFIER=dev.winfer.native.dev),
# so both can run at once.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build -p winfer-app --bin winfer-native
APP="target/debug/winfer-dev.app"
rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/debug/winfer-native "$APP/Contents/MacOS/winfer"
cp ../src-tauri/icons/icon.icns "$APP/Contents/Resources/icon.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>winfer dev</string>
  <key>CFBundleIdentifier</key><string>dev.winfer.native.dev</string>
  <key>CFBundleExecutable</key><string>winfer</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>winfer uses the microphone for voice input and live voice.</string>
  <key>NSSpeechRecognitionUsageDescription</key><string>winfer turns what you say into text when you dictate a message.</string>
</dict></plist>
PLIST
codesign --force --sign - "$APP"

args=(--env "WINFER_NATIVE_IDENTIFIER=${WINFER_NATIVE_IDENTIFIER:-dev.winfer.native.dev}")
while IFS='=' read -r name value; do
  [[ $name == WINFER_* && $name != WINFER_NATIVE_IDENTIFIER ]] && args+=(--env "$name=$value")
done < <(env)
open -n "$APP" "${args[@]}"
