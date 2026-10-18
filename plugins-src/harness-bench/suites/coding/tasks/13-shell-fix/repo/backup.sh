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

KEEP=3
if [ "$1" = "-k" ]; then
  KEEP=$2
  shift
fi
SRC=$1
DEST=$2

if [ -z "$SRC" ] || [ -z "$DEST" ]; then
  echo "usage: backup.sh [-k KEEP] SRC DEST"
  exit 1
fi

mkdir -p $DEST

i=$KEEP
while [ $i -gt 0 ]; do
  if [ -f $DEST/backup.$i.tar ]; then
    mv $DEST/backup.$i.tar $DEST/backup.$((i + 1)).tar
    mv $DEST/backup.$i.list $DEST/backup.$((i + 1)).list
  fi
  i=$((i - 1))
done

tar -C "$SRC" -cf "$DEST/backup.1.tar" . 2>/dev/null
: > "$DEST/backup.1.list"
for f in $(cd "$SRC" && find . -type f); do
  echo "${f#./}" >> "$DEST/backup.1.list"
done
sort -o "$DEST/backup.1.list" "$DEST/backup.1.list"
echo "backup done"
