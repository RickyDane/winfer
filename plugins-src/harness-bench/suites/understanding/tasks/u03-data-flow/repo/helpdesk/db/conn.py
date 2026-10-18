import sqlite3

_conn = None


def execute(sql, params=()):
    global _conn
    if _conn is None:
        _conn = sqlite3.connect("helpdesk.db")
    return _conn.execute(sql, params).fetchall()
