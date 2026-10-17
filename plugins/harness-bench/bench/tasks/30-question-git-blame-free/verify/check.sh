#!/bin/sh
[ -f ANSWER.txt ] || { echo "no ANSWER.txt"; exit 1; }
a=$(tr -d ' \n\r\t`"'"'" < ANSWER.txt)
[ "$a" = "net.retry.base_delay_ms" ] || { echo "answer: $a"; exit 1; }
other=$(git status --porcelain --untracked-files=all | grep -v -e ' ANSWER.txt$' -e '__pycache__/' -e '\.pyc$')
[ -z "$other" ] || { echo "other files changed:"; echo "$other"; exit 1; }
