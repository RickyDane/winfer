#!/bin/sh
fail() { echo "$1"; exit 1; }
[ -f .git/watch.log ] || fail "watcher never ran"
python3 - <<'PY' || fail "calc.py still wrong"
import calc
assert calc.average([1,2,3]) == 2 and calc.average([]) == 0
assert calc.clamp(5,0,10) == 5 and calc.clamp(-1,0,10) == 0 and calc.clamp(11,0,10) == 10
assert calc.percent(1,4) == 25 and calc.percent(0,0) == 0
PY
tail -1 .git/watch.log | grep -q "^7/7" || fail "the watcher never saw all tests pass"
if [ -f .git/watch.pid ] && kill -0 "$(cat .git/watch.pid)" 2>/dev/null; then kill "$(cat .git/watch.pid)"; fail "watcher left running"; fi
true
