#!/bin/sh
# Runs backup.sh in a temp dir with awkward names.
S="$(pwd)/backup.sh"
T=$(mktemp -d "${TMPDIR:-/tmp}/hb13.XXXXXX")
trap 'chmod -R u+rwx "$T" 2>/dev/null; rm -rf "$T"' EXIT
fail() { echo "$1"; exit 1; }
run() { timeout 30 sh "$S" "$@" >"$T/out" 2>"$T/err"; }

SRC="$T/my src"
DEST="$T/back ups/dest dir"
mkdir -p "$SRC/sub dir/[x]" "$SRC/empty dir"
printf 'v0' > "$SRC/gen.txt"
printf a > "$SRC/plain.txt"
printf b > "$SRC/with space.txt"
printf c > "$SRC/sub dir/-dash.txt"
printf d > "$SRC/sub dir/[x]/*star*.md"
printf e > "$SRC/.hidden"
printf f > "$SRC/sub dir/café  two.txt"
expected=$(cd "$SRC" && find . -type f | sed 's|^\./||' | LC_ALL=C sort)

for g in 1 2 3 4 5; do
  printf "v$g" > "$SRC/gen.txt"
  run -k 3 "$SRC" "$DEST" || fail "run $g exited $? ($(cat "$T/err"))"
done
got=$(cd "$DEST" && ls | LC_ALL=C sort | tr '\n' ' ')
[ "$got" = "backup.1.list backup.1.tar backup.2.list backup.2.tar backup.3.list backup.3.tar " ] || fail "with -k 3 after 5 runs DEST has: $got"
for n in 1 2 3; do
  want="v$((6 - n))"
  have=$(tar -xOf "$DEST/backup.$n.tar" ./gen.txt 2>/dev/null || tar -xOf "$DEST/backup.$n.tar" gen.txt 2>/dev/null)
  [ "$have" = "$want" ] || fail "backup.$n.tar has gen.txt=$have, want $want"
done
[ "$(cat "$DEST/backup.1.list")" = "$expected" ] || { printf '%s\n---\n%s\n' "$(cat "$DEST/backup.1.list")" "$expected"; fail "backup.1.list wrong"; }
mkdir "$T/x" && tar -C "$T/x" -xf "$DEST/backup.2.tar" || fail "backup.2.tar does not extract"
[ "$(cat "$T/x/sub dir/café  two.txt")$(cat "$T/x/sub dir/[x]/*star*.md")$(cat "$T/x/with space.txt")$(cat "$T/x/.hidden")" = fdbe ] || fail "archive contents wrong"
[ -d "$T/x/empty dir" ] || fail "archive misses empty directory"

# Default KEEP is 3; -k 1 keeps one.
D2="$T/d2"
for g in 1 2 3 4; do run "$SRC" "$D2" || fail "default run failed"; done
[ "$(ls "$D2" | wc -l | tr -d ' ')" = 6 ] || fail "default keep is not 3: $(ls "$D2" | tr '\n' ' ')"
run -k 1 "$SRC" "$D2" || fail "-k 1 failed"
[ "$(ls "$D2" | LC_ALL=C sort | tr '\n' ' ')" = "backup.1.list backup.1.tar " ] || fail "-k 1 left: $(ls "$D2" | tr '\n' ' ')"

# Usage errors exit 2 with a message on stderr.
for args in "" "$SRC" "-k" "-k 0 SRC DEST" "-k x SRC DEST" "-k -1 SRC DEST" "NOPE DEST" "SRC DEST extra"; do
  set -- $args
  a=""; for w in "$@"; do case $w in SRC) w=$SRC ;; DEST) w="$T/u" ;; NOPE) w="$T/nope" ;; esac; a="$a${a:+
}$w"; done
  old=$IFS; IFS='
'; set -- $a; IFS=$old
  run "$@"; code=$?
  [ $code -eq 2 ] || fail "args '$args' exited $code, want 2"
  [ -s "$T/err" ] || fail "args '$args': no usage message on stderr"
done
[ ! -e "$T/u" ] || fail "DEST created on usage error"

# Archiving failure: exit 1, old backups untouched. Only where chmod 000 makes a file
# unreadable: not as root, nor on Windows (Git's sh maps chmod to the read-only attribute).
before=$(cd "$DEST" && ls -l | tr -s ' ' | cut -d' ' -f5,9- ; cksum backup.1.tar backup.3.tar)
printf secret > "$SRC/locked.txt"; chmod 000 "$SRC/locked.txt"
if cat "$SRC/locked.txt" >/dev/null 2>&1; then
  chmod 644 "$SRC/locked.txt"
else
  run -k 3 "$SRC" "$DEST"; code=$?
  chmod 644 "$SRC/locked.txt"
  [ $code -eq 1 ] || fail "archiving an unreadable file exited $code, want 1"
  after=$(cd "$DEST" && ls -l | tr -s ' ' | cut -d' ' -f5,9- ; cksum backup.1.tar backup.3.tar)
  [ "$before" = "$after" ] || fail "existing backups changed after a failed run"
fi
exit 0
