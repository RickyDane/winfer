import glob
import os
import sqlite3
import subprocess
import sys
import tempfile

sys.path.insert(0, os.getcwd())


def fail(msg):
    print(msg)
    sys.exit(1)


def split(name):
    s = name.strip()
    if " " not in s:
        return s, ""
    i = s.rfind(" ")
    return s[:i].strip(), s[i + 1:]


if subprocess.run(["git", "diff", "--quiet", "HEAD", "--", "migrations/001_init.sql"]).returncode:
    fail("001_init.sql was modified")
if not glob.glob("migrations/002_*.sql") + glob.glob("migrations/002_*.py"):
    fail("no migrations/002_* file")

from contacts import db, repo  # noqa: E402

tmp = tempfile.mkdtemp()
path = os.path.join(tmp, "seed.db")
with open(path, "wb") as f:
    f.write(subprocess.run(["git", "show", "HEAD:data/contacts.db"], capture_output=True, check=True).stdout)
conn = sqlite3.connect(path)
tricky = ["Ludwig van Beethoven", "  Cher  ", "Ann  Lee", "Prince", "José María  Pérez Gómez", "O'Neil  ", "A B"]
for i, n in enumerate(tricky):
    conn.execute("INSERT INTO people (id, name, email, created_at) VALUES (?, ?, ?, ?)", (900 + i, n, f"t{i}@x.io", "2020-01-01 00:00:00"))
conn.commit()
before = conn.execute("SELECT id, name, email, created_at FROM people ORDER BY id").fetchall()
conn.close()

conn = db.connect(path)
try:
    v = db.migrate(conn)
except Exception as e:
    fail(f"migration failed: {e!r}")
if v != 2 or conn.execute("PRAGMA user_version").fetchone()[0] != 2:
    fail(f"schema version {v}, expected 2")
cols = [r[1] for r in conn.execute("PRAGMA table_info(people)")]
if "name" in cols:
    fail("name column still exists")
for c in ("id", "first_name", "last_name", "email", "created_at"):
    if c not in cols:
        fail(f"missing column {c}")
after = conn.execute("SELECT id, first_name, last_name, email, created_at FROM people ORDER BY id").fetchall()
if len(after) != len(before):
    fail(f"{len(before)} rows before, {len(after)} after")
for (i, name, email, created), row in zip(before, after):
    want = (i, *split(name), email, created)
    if tuple(row) != want:
        fail(f"row {i} {name!r}: got {tuple(row)}, want {want}")
idx = {r[1]: r for r in conn.execute("PRAGMA index_list(people)")}
if "idx_people_last_first" not in idx:
    fail("index idx_people_last_first missing")
icols = [r[2] for r in conn.execute("PRAGMA index_info(idx_people_last_first)")]
if icols != ["last_name", "first_name"]:
    fail(f"index columns {icols}")
if "idx_people_email" not in idx:
    fail("idx_people_email was lost")
if db.migrate(conn) != 2:
    fail("re-running migrate changed the version")

# App code on the migrated db.
pid = repo.add_person(conn, "Zed", "Lee", "z@x.io")
repo.add_person(conn, "Abe", "Lee")
repo.add_person(conn, "Solo", "")
p = repo.get_person(conn, pid)
if (p["first_name"], p["last_name"], p["email"]) != ("Zed", "Lee", "z@x.io"):
    fail("add_person stored wrong values")
lees = repo.find_by_last_name(conn, "Lee")
firsts = [r["first_name"] for r in lees]
if firsts != sorted(firsts) or "Zed" not in firsts or "Abe" not in firsts or any(r["last_name"] != "Lee" for r in lees):
    fail(f"find_by_last_name wrong: {firsts}")
if repo.find_by_last_name(conn, "ee"):
    fail("find_by_last_name is not an exact match")
rows = repo.list_people(conn)
keys = [(r["last_name"], r["first_name"], r["id"]) for r in rows]
if keys != sorted(keys) or len(rows) != len(after) + 3:
    fail("list_people order wrong")
names = {r["id"]: repo.display_name(r) for r in rows}
if names[pid] != "Zed Lee" or names[900] != "Ludwig van Beethoven" or names[901] != "Cher" or names[902] != "Ann Lee":
    fail(f"display_name wrong: {names[pid]!r} {names[900]!r} {names[901]!r} {names[902]!r}")
conn.close()

# Fresh database straight to v2.
fresh = db.connect(os.path.join(tmp, "fresh.db"))
if db.migrate(fresh) != 2:
    fail("fresh migrate failed")
fresh.close()

# CLI.
cli = lambda *a: subprocess.run([sys.executable, "-m", "contacts.cli", "--db", os.path.join(tmp, "fresh.db"), *a], capture_output=True, text=True)
r = cli("add", "Ada", "Lovelace", "--email", "ada@x.io")
if r.returncode:
    fail("cli add failed: " + r.stderr[-300:])
r = cli("find", "Lovelace")
if "Ada Lovelace" not in r.stdout or "ada@x.io" not in r.stdout:
    fail("cli find output: " + (r.stdout + r.stderr)[-300:])

# The shipped database is migrated too.
conn = sqlite3.connect("data/contacts.db")
if conn.execute("PRAGMA user_version").fetchone()[0] != 2:
    fail("data/contacts.db not migrated")
n = conn.execute("SELECT count(*) FROM people WHERE last_name = 'Smith'").fetchone()[0]
if n == 0:
    fail("data/contacts.db has no split data")
