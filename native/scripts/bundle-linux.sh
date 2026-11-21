#!/usr/bin/env bash
# Builds the native GPUI build for Linux as target/bundle/winfer-native-linux-<arch>.tar.gz: the
# winfer binary, which is also the updater bundle installed apps unpack over themselves (updates.rs).
# Needs WebKitGTK 4.1, GTK 3 and ALSA at run time.
# Usage: native/scripts/bundle-linux.sh [x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu]
set -euo pipefail
cd "$(dirname "$0")/.."
TARGET="${1:-$(rustc -vV | sed -n 's/host: //p')}"
OUT="target/bundle"
STAGE="$OUT/linux-${TARGET%%-*}"
ARCHIVE="$OUT/winfer-native-linux-${TARGET%%-*}.tar.gz"
cargo build --release -p winfer-app --bin winfer-native --target "$TARGET"
rm -rf "$STAGE" "$ARCHIVE" && mkdir -p "$STAGE"
install -m 755 "target/$TARGET/release/winfer-native" "$STAGE/winfer"
tar -czf "$ARCHIVE" -C "$STAGE" winfer
rm -rf "$STAGE"
echo "$ARCHIVE"
