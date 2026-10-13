#!/usr/bin/env bash
# Start the best-config server, then: cache test, two-stage eval, compact eval, full eval. Stops the server at the end.
set -uo pipefail
cd "$(dirname "$0")"
[[ -f server.pid ]] && kill "$(cat server.pid)" 2>/dev/null; sleep 2
nohup ./run_server.sh > results/server_eval.log 2>&1 &
echo $! > server.pid
for i in $(seq 1 60); do grep -qE 'listening on|exiting' results/server_eval.log && break; sleep 1; done
( while kill -0 "$(cat server.pid)" 2>/dev/null; do echo "$(date +%s) $(ps -o rss= -p "$(cat server.pid)")"; sleep 2; done ) > results/rss_eval.txt &
echo "== cache"; python3 eval.py cache --n 4 --size s
for size in twostage compact full; do
  echo "== eval $size"; python3 eval.py eval --size "$size" > "results/eval_${size}.log" 2>&1; tail -3 "results/eval_${size}.log"
done
grep -c 'cached n_tokens = 0' results/server_eval.log | sed 's/^/requests with cached n_tokens = 0: /'
grep -E 'cached n_tokens = [1-9]' results/server_eval.log | head -3
kill "$(cat server.pid)"; echo PIPELINE_DONE
