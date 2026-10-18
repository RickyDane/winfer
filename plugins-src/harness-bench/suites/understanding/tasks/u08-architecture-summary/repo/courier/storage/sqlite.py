import sqlite3

from .base import Store


class SqliteStore(Store):
    def __init__(self, settings):
        super().__init__(settings)
        self.path = settings.get("storage.path", ":memory:")
        self._conn = None

    def _db(self):
        if self._conn is None:
            self._conn = sqlite3.connect(self.path)
            self._conn.execute("CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT)")
        return self._conn

    def put(self, key, value):
        self._db().execute("INSERT OR REPLACE INTO kv VALUES (?, ?)", (key, value))

    def get(self, key):
        row = self._db().execute("SELECT v FROM kv WHERE k = ?", (key,)).fetchone()
        return row and row[0]
