#!/bin/sh
# Installs the agents named in WINFERD_AGENTS (claude, codex, gemini) into the volume on first
# start, then runs winferd.
set -e
mkdir -p "$HOME"
for agent in $(echo "${WINFERD_AGENTS:-}" | tr ',' ' '); do
  case "$agent" in
    claude) pkg=@anthropic-ai/claude-code; bin=claude ;;
    codex) pkg=@openai/codex; bin=codex ;;
    gemini) pkg=@google/gemini-cli; bin=gemini ;;
    *) echo "WINFERD_AGENTS: unknown agent $agent" >&2; continue ;;
  esac
  command -v "$bin" >/dev/null 2>&1 || { echo "installing $agent…" >&2; npm install -g "$pkg" >&2 || echo "couldn't install $agent" >&2; }
done
exec winferd "$@"
