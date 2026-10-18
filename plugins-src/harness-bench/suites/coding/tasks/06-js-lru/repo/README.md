# lru

`new LRUCache({ max, ttl = 0, now = Date.now })` — a least-recently-used cache.

- `max` (integer ≥ 1, required): most entries kept. Inserting into a full cache evicts the least recently used entry. Throws `RangeError` if invalid.
- `ttl` (ms, 0 = never): entries expire `ttl` ms after they were last **set** (reads don't extend it). Expired entries behave as absent.
- `now`: clock function, injectable for tests.

Methods:
- `set(key, value, { ttl } = {})` → the cache (chainable). A per-entry `ttl` overrides the default. Setting an existing key updates it and makes it most recent.
- `get(key)` → value or `undefined`; marks the entry most recently used.
- `peek(key)` → value or `undefined` **without** changing recency.
- `has(key)` → boolean, doesn't change recency.
- `delete(key)` → boolean (whether it existed and wasn't expired).
- `clear()`.
- `size` (getter) → number of non-expired entries.
- `keys()` → array of non-expired keys, most recently used first.
- `onEvict` option `(key, value, reason)` called with reason `"evict"` (capacity) or `"expire"` (found expired on access, on `size`/`keys()`, or when making room). Not called for `delete`/`clear`/overwrite.

When the cache is full, expired entries are removed (reason `"expire"`) before evicting a live one.
