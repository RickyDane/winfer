#!/bin/sh
fail() { echo "$1"; exit 1; }
export PYTHONDONTWRITEBYTECODE=1
git diff --quiet HEAD -- tests app/cache.py || fail "tests/ or app/cache.py was modified"
# Old client.fetch calls (any receiver except the cache), single- or multi-line.
left=$(grep -rnE '\.fetch\(' --include=*.py app | grep -v '_cache\.fetch(' | grep -v 'def fetch(self, key')
[ -z "$left" ] || { echo "$left" | head -5; fail "fetch( calls remain"; }
python3 -c "from app.http import Client; assert not hasattr(Client, 'fetch')" 2>/dev/null || fail "Client.fetch still exists"
grep -q "def fetch_all(self, urls, timeout)" app/http/client.py || fail "fetch_all was changed or removed"
count() { grep -rE "$1" --include=*.py app | grep -v '^app/http/client.py' | wc -l | tr -d ' '; }
was() { git grep -E "$1" HEAD -- 'app/*.py' | grep -v ':app/http/client.py:' | wc -l | tr -d ' '; }
[ "$(count 'fetch_all\(')" = "$(was 'fetch_all\(')" ] || fail "fetch_all call sites changed"
[ "$(count '_cache\.fetch\(')" = "$(was '_cache\.fetch\(')" ] || fail "Cache.fetch call sites changed"
n=$(was '(client|_http|api)\.fetch\(')
[ "$(count '\.request\(')" = "$n" ] || fail "expected $n request( call sites, found $(count '\.request\(')"
python3 -m unittest discover -s tests -q 2>&1 || fail "tests fail"
python3 - <<'PY' || fail "fetch_all behavior changed"
from app.http import Client
calls = []
c = Client(lambda *a: calls.append(a) or len(calls))
assert c.fetch_all(["a", "b"], 3) == [1, 2]
assert calls == [("GET", "a", 3, None), ("GET", "b", 3, None)]
PY
