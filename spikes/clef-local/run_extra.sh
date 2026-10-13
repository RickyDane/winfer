#!/usr/bin/env bash
set -uo pipefail
cd "$(dirname "$0")"
nohup ./run_server.sh > results/server_extra.log 2>&1 &
echo $! > server.pid
for i in $(seq 1 60); do grep -qE 'listening on|exiting' results/server_extra.log && break; sleep 1; done
python3 tokcount.py s_rich
echo "== eval s_rich"; python3 eval.py eval --size s_rich > results/eval_s_rich.log 2>&1; grep -E '"action_1_acc"|"p50"' results/eval_s_rich.log | head -3
echo "== count variants"; python3 count_variants.py > results/count_variants.log 2>&1; grep -E '^\S+ [0-9]+/' results/count_variants.log
kill "$(cat server.pid)"; echo EXTRA_DONE
