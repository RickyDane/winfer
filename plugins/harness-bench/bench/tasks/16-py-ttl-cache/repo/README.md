# ttlcache

`from ttlcache import ttl_cache, CacheInfo`

```python
@ttl_cache(seconds=30, maxsize=100)
def lookup(user_id, *, region="eu"):
    ...
```

## `ttl_cache(seconds, maxsize=128, clock=None)`

Returns a decorator that memoizes a function's results for `seconds` seconds.

- `seconds` must be a positive number, otherwise `ValueError` (raised when `ttl_cache(...)` is
  called, not later).
- `maxsize` must be a positive int or `None` (unbounded), otherwise `ValueError`.
- `clock` is a zero-argument function returning the current time in seconds (float); it
  defaults to `time.monotonic`. Tests inject a fake clock.

### Keys

The cache key is built from the positional arguments and the keyword arguments. Keyword
argument order doesn't matter (`f(a=1, b=2)` and `f(b=2, a=1)` share an entry), but a value
passed positionally and the same value passed by keyword are different entries (no signature
binding). `f(1)` and `f(1.0)` are the **same** entry (normal dict equality). Calling with an
unhashable argument raises `TypeError` (don't call the function).

### Expiry

An entry stored at time `t` is valid while `clock() < t + seconds`; at `clock() >= t + seconds`
it is expired and the next call recomputes (counted as a miss) and stores a fresh entry.
Exceptions raised by the function are not cached.

### Size

When storing a new entry would exceed `maxsize`, expired entries are dropped first; if it is
still full, the **least recently used** entry is evicted (a cache hit counts as a use).

### Attributes on the wrapped function

- `cache_info()` returns `CacheInfo(hits, misses, maxsize, currsize)` (a namedtuple exported by
  the module). `currsize` counts only entries that are not expired at the time of the call.
- `cache_clear()` empties the cache and resets hits and misses to 0.
- `__wrapped__`, `__name__`, `__doc__` as with `functools.wraps`.

### Threads

The wrapper must be safe to call from many threads at once: counters stay exact
(`hits + misses` equals the number of calls) and the cache never grows past `maxsize`.
Calls for **the same key** that arrive while that key is being computed wait for that
computation instead of calling the function again (so a slow function called concurrently
with the same arguments runs once, and the calls that waited count as hits). Calls for different keys must not block each other while
the function runs.
