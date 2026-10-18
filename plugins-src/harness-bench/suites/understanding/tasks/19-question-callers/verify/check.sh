#!/bin/sh
[ -f ANSWER.txt ] || { echo "no ANSWER.txt"; exit 1; }
a=$(tr -d ' \n\r`' < ANSWER.txt)
[ "$a" = "34" ] || { echo "answer: $a"; exit 1; }
[ -z "$(git status --porcelain | grep -v ANSWER.txt)" ] || { echo "other files changed"; git status --porcelain; exit 1; }
