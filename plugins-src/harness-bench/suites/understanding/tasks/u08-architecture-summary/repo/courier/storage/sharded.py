"""SQLite sharded by key hash. Replaces the plain SQLite backend when sharding is on."""

import os
import zlib

from . import BACKENDS
from .sqlite import SqliteStore

SHARDS = int(os.environ.get("COURIER_SHARDS", "4"))


class ShardedSqliteStore(SqliteStore):
    def __init__(self, settings):
        super().__init__(settings)
        self.shards = [SqliteStore(settings) for _ in range(SHARDS)]

    def _shard(self, key):
        return self.shards[zlib.crc32(key.encode()) % SHARDS]

    def put(self, key, value):
        self._shard(key).put(key, value)

    def get(self, key):
        return self._shard(key).get(key)


if SHARDS > 1:
    BACKENDS["sqlite"] = ShardedSqliteStore
