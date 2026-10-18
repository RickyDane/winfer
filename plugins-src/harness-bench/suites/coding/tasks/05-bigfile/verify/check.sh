#!/bin/sh
[ -f report.py ] || { echo "no report.py"; exit 1; }
[ -f REPORT.txt ] || { echo "no REPORT.txt"; exit 1; }
expected=$(grep ' ERROR ' logs/app.log | grep -o 'code=E[0-9]*' | cut -d= -f2 | sort | uniq -c | sort -k1,1nr -k2 | head -3 | awk '{print $2" "$1}')
got=$(sed '/^[[:space:]]*$/d' REPORT.txt)
[ "$got" = "$expected" ] || { printf 'expected:\n%s\ngot:\n%s\n' "$expected" "$got"; exit 1; }
out=$(python3 report.py | sed '/^[[:space:]]*$/d')
[ "$out" = "$expected" ] || { echo "report.py output differs from REPORT.txt"; exit 1; }
