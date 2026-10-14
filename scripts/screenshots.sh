#!/usr/bin/env bash
# Captures the README screenshots from the dev server's showcase mode (src/dev/showcase.ts).
# Run `pnpm dev` first. Needs Google Chrome (or set CHROME=/path/to/chrome).
set -euo pipefail
cd "$(dirname "$0")/.."
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
URL="${URL:-http://localhost:1420}"
OUT=docs/screenshots
mkdir -p "$OUT"
for scene in "${@:-hero overview review usage sessions light}"; do
  for s in $scene; do
    "$CHROME" --headless=new --hide-scrollbars --force-device-scale-factor=2 --window-size=1760,1040 \
      --virtual-time-budget=10000 --user-data-dir="$(mktemp -d)" \
      --screenshot="$OUT/$s.png" "$URL/?showcase=$s" 2>/dev/null
    echo "$OUT/$s.png"
  done
done
