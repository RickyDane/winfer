#!/bin/sh
# Usage: backup.sh [-k KEEP] SRC DEST
#
# Archives the directory SRC into DEST/backup.1.tar (tar of SRC's contents, paths
# relative to SRC) and writes DEST/backup.1.list: the relative paths of all regular files
# in SRC (no leading "./"), one per line, sorted with LC_ALL=C sort.
#
# Existing backups are rotated first: backup.N.tar/.list becomes backup.N+1.tar/.list.
# Afterwards at most KEEP backups (default 3) exist: backup.1 (newest) .. backup.KEEP;
# older ones are deleted. DEST is created if missing.
#
# Exit status: 0 on success; 2 (with a usage message on stderr) when the arguments are
# wrong, SRC is not a directory or KEEP is not a positive integer; 1 if archiving fails,
# in which case the existing backups are left exactly as they were.
# File names never contain newlines.

usage() {
  echo "usage: backup.sh [-k KEEP] SRC DEST" >&2
  exit 2
}

KEEP=3
if [ "$1" = "-k" ]; then
  [ $# -ge 2 ] || usage
  KEEP=$2
  shift 2
fi
[ $# -eq 2 ] || usage
SRC=$1
DEST=$2
case $KEEP in
  '' | *[!0-9]*) usage ;;
esac
[ "$KEEP" -ge 1 ] || usage
[ -d "$SRC" ] || usage

mkdir -p "$DEST" || exit 1

tmp_tar="$DEST/.backup.tmp.$$.tar"
tmp_list="$DEST/.backup.tmp.$$.list"
cleanup() { rm -f "$tmp_tar" "$tmp_list"; }

if ! tar -C "$SRC" -cf "$tmp_tar" . 2>/dev/null; then
  cleanup
  echo "backup.sh: archiving $SRC failed" >&2
  exit 1
fi
if ! (cd "$SRC" && find . -type f) | sed 's|^\./||' | LC_ALL=C sort > "$tmp_list"; then
  cleanup
  exit 1
fi

# Rotate: drop everything from KEEP up, then shift the rest.
for f in "$DEST"/backup.*.tar "$DEST"/backup.*.list; do
  [ -e "$f" ] || continue
  n=${f##*/backup.}
  n=${n%.*}
  case $n in '' | *[!0-9]*) continue ;; esac
  [ "$n" -ge "$KEEP" ] && rm -f "$f"
done
i=$((KEEP - 1))
while [ "$i" -ge 1 ]; do
  for ext in tar list; do
    [ -e "$DEST/backup.$i.$ext" ] && mv "$DEST/backup.$i.$ext" "$DEST/backup.$((i + 1)).$ext"
  done
  i=$((i - 1))
done

mv "$tmp_tar" "$DEST/backup.1.tar" && mv "$tmp_list" "$DEST/backup.1.list" || exit 1
exit 0
