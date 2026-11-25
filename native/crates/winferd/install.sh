#!/bin/sh
# Installs winferd (winfer for servers) for the current user, runs it as a service and pairs it.
#
#   curl -fsSL https://winfer.danehub.com/install.sh | sh -s -- --pair '<link from winfer: Settings → Connect>'
#
# Options:
#   --pair <link>     pair with the winfer that made the link (prints the 6-digit code)
#   --name <name>     this server's name in winfer (default: the host name)
#   --version <tag>   a release tag, e.g. v0.1.102 (default: the latest release)
#   --no-service      install only; don't set up the systemd/launchd service
# Installs to ~/.local/bin (WINFERD_BIN_DIR to change). Needs curl or wget, tar, and sha256sum or shasum.
set -eu

REPO="RickyDane/winfer"
PAIR=""
NAME=""
VERSION="${WINFERD_VERSION:-}"
SERVICE=1
BIN_DIR="${WINFERD_BIN_DIR:-$HOME/.local/bin}"

while [ $# -gt 0 ]; do
  case "$1" in
    --pair) PAIR="$2"; shift 2 ;;
    --name) NAME="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    --no-service) SERVICE=0; shift ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

say() { printf '%s\n' "$*" >&2; }
die() { say "error: $*"; exit 1; }

case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=macos ;;
  *) die "winferd runs on Linux and macOS (this is $(uname -s))" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) die "unsupported CPU $(uname -m)" ;;
esac
asset="winferd-$os-$arch.tar.gz"
if [ -n "$VERSION" ]; then
  base="https://github.com/$REPO/releases/download/$VERSION"
else
  base="https://github.com/$REPO/releases/latest/download"
fi
base="${WINFERD_BASE_URL:-$base}"

fetch() {
  if command -v curl >/dev/null 2>&1; then curl -fsSL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then wget -qO "$2" "$1"
  else die "needs curl or wget"; fi
}
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
say "Downloading $asset…"
fetch "$base/$asset" "$tmp/$asset" || die "couldn't download $base/$asset"
fetch "$base/$asset.sha256" "$tmp/$asset.sha256" || die "couldn't download the checksum"
[ "$(sha256 "$tmp/$asset")" = "$(tr -d ' \n' < "$tmp/$asset.sha256")" ] || die "checksum mismatch: not installing"
tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$BIN_DIR"
# Replace rather than overwrite: a running winferd keeps its old file.
install -m 0755 "$tmp/winferd" "$BIN_DIR/winferd.new" 2>/dev/null || { cp "$tmp/winferd" "$BIN_DIR/winferd.new"; chmod 0755 "$BIN_DIR/winferd.new"; }
mv -f "$BIN_DIR/winferd.new" "$BIN_DIR/winferd"
[ "$os" = macos ] && xattr -d com.apple.quarantine "$BIN_DIR/winferd" 2>/dev/null || true
say "Installed $("$BIN_DIR/winferd" --version) to $BIN_DIR/winferd"
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) say "Add $BIN_DIR to your PATH to run winferd by name." ;; esac

if [ -n "$NAME" ]; then
  # The name is read at startup; keep it for the service.
  mkdir -p "$HOME/.config/winferd"
  printf '%s\n' "$NAME" > "$HOME/.config/winferd/name"
fi

if [ "$SERVICE" = 1 ]; then
  if [ "$os" = linux ] && ! command -v systemctl >/dev/null 2>&1; then
    say "No systemd here: start it yourself with: $BIN_DIR/winferd run"
  else
    "$BIN_DIR/winferd" service install || say "Couldn't set up the service; start it with: $BIN_DIR/winferd run"
    sleep 2
  fi
fi

if [ -n "$PAIR" ]; then
  "$BIN_DIR/winferd" pair "$PAIR"
fi
