#!/usr/bin/env bash
# Restart llama-server with each config and run bench_cfg.py against it.
# Usage: nohup ./sweep.sh "label|extra args" ... > results/sweep.log 2>&1 &
set -uo pipefail
cd "$(dirname "$0")"
B="${HOME}/.cache/winfer-spike/llama.cpp/build/bin/llama-server"
M="${HOME}/.cache/winfer-spike/models/clef-flash/Clef-Flash-Q4_K_M.gguf"
KEY="$(cat .api_key)"
N="${N:-3}"
ONLY="${ONLY:-floor,syn500,syn1k,syn2k,real_compact,real_full}"
for spec in "$@"; do
  label="${spec%%|*}"; args="${spec#*|}"
  [[ -f server.pid ]] && kill "$(cat server.pid)" 2>/dev/null; sleep 2
  # shellcheck disable=SC2086
  "$B" -m "$M" --host 127.0.0.1 --port 8899 --api-key "$KEY" -ngl 99 -np 1 --metrics --no-webui --alias clef-flash $args \
     > "results/server_${label}.log" 2>&1 &
  echo $! > server.pid
  for i in $(seq 1 60); do grep -qE 'listening on|exiting|error' "results/server_${label}.log" && break; sleep 1; done
  echo "== $label: $args"; grep -E 'n_batch|n_ubatch|flash_attn|Flash Attention|compute buffer|exiting|error' "results/server_${label}.log" | sed 's/^/   /'
  ONLY_ARG="$ONLY"
  python3 bench_cfg.py "$label" --n "$N" --only "$ONLY_ARG"
  echo "   rss_kb=$(ps -o rss= -p "$(cat server.pid)")"
done
echo SWEEP_DONE
