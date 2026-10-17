"""Generates the contacts project (schema v1) and a seeded data/contacts.db into argv[1]."""

import os
import random
import sqlite3
import sys

root = sys.argv[1]
rng = random.Random(12)


def write(path, text):
    full = os.path.join(root, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w") as f:
        f.write(text)


write(".gitignore", "__pycache__/\n")
write("contacts/__init__.py", "")
write(
    "migrations/001_init.sql",
    """CREATE TABLE people (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    email TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_people_email ON people(email);
""",
)
write(
    "contacts/db.py",
    '''"""Connection and migrations.

Migrations live in migrations/ as NNN_name.sql (a SQL script) or NNN_name.py (a module with
`up(conn)`). They run in number order; PRAGMA user_version records the last one applied.
"""

import importlib.util
import os
import re
import sqlite3

MIGRATIONS = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "migrations")


def connect(path):
    conn = sqlite3.connect(path)
    conn.row_factory = sqlite3.Row
    return conn


def migrations(directory=MIGRATIONS):
    found = []
    for name in sorted(os.listdir(directory)):
        m = re.match(r"(\\d{3})_\\w+\\.(sql|py)$", name)
        if m:
            found.append((int(m.group(1)), os.path.join(directory, name)))
    return found


def migrate(conn, directory=MIGRATIONS):
    """Applies pending migrations; returns the new schema version."""
    version = conn.execute("PRAGMA user_version").fetchone()[0]
    for number, path in migrations(directory):
        if number <= version:
            continue
        if path.endswith(".sql"):
            with open(path) as f:
                script = f.read()
            conn.executescript(f"BEGIN;\\n{script}\\nPRAGMA user_version = {number};\\nCOMMIT;")
        else:
            spec = importlib.util.spec_from_file_location(f"migration_{number}", path)
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            conn.execute("BEGIN")
            try:
                module.up(conn)
                conn.execute(f"PRAGMA user_version = {number}")
                conn.commit()
            except Exception:
                conn.rollback()
                raise
        version = number
    return version
''',
)
write(
    "contacts/repo.py",
    '''"""Queries on people."""


def add_person(conn, name, email=None):
    cur = conn.execute("INSERT INTO people (name, email) VALUES (?, ?)", (name, email))
    conn.commit()
    return cur.lastrowid


def get_person(conn, person_id):
    return conn.execute("SELECT * FROM people WHERE id = ?", (person_id,)).fetchone()


def list_people(conn):
    return conn.execute("SELECT * FROM people ORDER BY name, id").fetchall()


def find_by_last_name(conn, last_name):
    """People whose name ends with `last_name`."""
    return conn.execute(
        "SELECT * FROM people WHERE name LIKE ? ORDER BY name, id", ("% " + last_name,)
    ).fetchall()


def display_name(row):
    return row["name"]
''',
)
write(
    "contacts/cli.py",
    '''"""python3 -m contacts.cli [--db PATH] (migrate | add NAME [--email E] | list | find LAST)"""

import argparse

from contacts import db, repo


def main(argv=None):
    p = argparse.ArgumentParser(prog="contacts")
    p.add_argument("--db", default="data/contacts.db")
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("migrate")
    a = sub.add_parser("add")
    a.add_argument("name")
    a.add_argument("--email")
    sub.add_parser("list")
    f = sub.add_parser("find")
    f.add_argument("last")
    args = p.parse_args(argv)

    conn = db.connect(args.db)
    if args.cmd == "migrate":
        print(f"schema version {db.migrate(conn)}")
    elif args.cmd == "add":
        print(repo.add_person(conn, args.name, args.email))
    else:
        rows = repo.list_people(conn) if args.cmd == "list" else repo.find_by_last_name(conn, args.last)
        for row in rows:
            print(f"{row['id']}\\t{repo.display_name(row)}\\t{row['email'] or ''}")


if __name__ == "__main__":
    main()
''',
)

first = ["Ann", "Bob", "Chen", "Dana", "Eli", "Fatima", "Gus", "Hana", "Ivan", "Jo", "Mary Ann", "Jean Luc"]
last = ["Smith", "Lee", "van Dyke", "Okafor", "Nguyen", "O'Brien", "de la Cruz", "Müller", "Kim"]
os.makedirs(os.path.join(root, "data"), exist_ok=True)
conn = sqlite3.connect(os.path.join(root, "data/contacts.db"))
conn.executescript(open(os.path.join(root, "migrations/001_init.sql")).read() + "PRAGMA user_version = 1;")
for i in range(150):
    name = f"{rng.choice(first)} {rng.choice(last)}"
    if i % 17 == 0:
        name = rng.choice(first).split()[0]
    if i % 11 == 0:
        name = "  " + name + " "
    email = None if i % 7 == 0 else f"p{i}@example.com"
    conn.execute(
        "INSERT INTO people (name, email, created_at) VALUES (?, ?, ?)",
        (name, email, f"2023-{1 + i % 12:02d}-{1 + i % 28:02d} 10:00:00"),
    )
conn.commit()
conn.close()
