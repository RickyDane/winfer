#!/usr/bin/env bash
# M0.5 spike: run llama-server with Clef-flash for /v1/systemone (local only).
# Usage: ./run_server.sh            (foreground)
# Env overrides: LLAMA_SERVER, MODEL, PORT, API_KEY, CTX, UBATCH
set -euo pipefail

CACHE="${HOME}/.cache/winfer-spike"
LLAMA_SERVER="${LLAMA_SERVER:-${CACHE}/llama.cpp/build/bin/llama-server}"
MODEL="${MODEL:-${CACHE}/models/clef-flash/Clef-Flash-Q4_K_M.gguf}"
PORT="${PORT:-8899}"
CTX="${CTX:-4096}"
# Clef evaluates the whole prompt in ONE batch: prompt must fit in --ubatch-size (default 512 is too small).
UBATCH="${UBATCH:-4096}"
KEY_FILE="$(dirname "$0")/.api_key"
if [[ -z "${API_KEY:-}" ]]; then
  [[ -f "$KEY_FILE" ]] || openssl rand -hex 24 > "$KEY_FILE"
  API_KEY="$(cat "$KEY_FILE")"
fi

exec "$LLAMA_SERVER" \
  -m "$MODEL" \
  --host 127.0.0.1 --port "$PORT" \
  --api-key "$API_KEY" \
  -ngl 99 \
  -c "$CTX" -b "$UBATCH" -ub "$UBATCH" \
  -np 1 \
  --metrics \
  --no-webui \
  --alias clef-flash \
  "$@"
