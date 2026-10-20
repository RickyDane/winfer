#!/bin/sh
fail() { echo "$1"; exit 1; }
[ -f .git/access.log ] || fail "server never started"
grep -q "^GET /api/secret" .git/access.log || fail "never fetched /api/secret from the server"
token=$(sed -n 's/^token //p' .git/access.log | head -1)
[ "$(tr -d ' \n' < answer.txt 2>/dev/null)" = "$token" ] || fail "answer.txt doesn't hold the token"
if [ -f .git/server.pid ] && kill -0 "$(cat .git/server.pid)" 2>/dev/null; then kill "$(cat .git/server.pid)"; fail "server left running"; fi
true
