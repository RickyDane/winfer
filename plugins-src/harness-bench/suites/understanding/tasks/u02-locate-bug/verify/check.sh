#!/bin/sh
[ -f ANSWER.txt ] || { echo "no ANSWER.txt"; exit 1; }
a=$(tr -d ' \n\r\t`"'"'" < ANSWER.txt | sed 's#^\./##')
[ "$a" = "billing/models.py:34" ] || { echo "answer: $a"; exit 1; }
other=$(git status --porcelain --untracked-files=all | grep -v -e ' ANSWER.txt$' -e '__pycache__/' -e '\.pyc$')
[ -z "$other" ] || { echo "other files changed:"; echo "$other"; exit 1; }
