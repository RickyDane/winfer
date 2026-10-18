"""Queries on people."""


def add_person(conn, first_name, last_name, email=None):
    cur = conn.execute(
        "INSERT INTO people (first_name, last_name, email) VALUES (?, ?, ?)", (first_name, last_name, email)
    )
    conn.commit()
    return cur.lastrowid


def get_person(conn, person_id):
    return conn.execute("SELECT * FROM people WHERE id = ?", (person_id,)).fetchone()


def list_people(conn):
    return conn.execute("SELECT * FROM people ORDER BY last_name, first_name, id").fetchall()


def find_by_last_name(conn, last_name):
    return conn.execute(
        "SELECT * FROM people WHERE last_name = ? ORDER BY first_name, id", (last_name,)
    ).fetchall()


def display_name(row):
    return f"{row['first_name']} {row['last_name']}" if row["last_name"] else row["first_name"]
