#!/bin/sh
# PreToolUse hook: deny Bash calls of the codegraph CLI (both arms), like codegraph's own benchmark.
python3 -I -c 'import json,sys; c=str(json.load(sys.stdin).get("tool_input",{}).get("command","")); sys.exit(2 if "codegraph" in c.lower() else 0)' && exit 0
echo "The codegraph CLI is not available in this environment." >&2; exit 2
