"""TTL cache decorator. See README.md."""

import functools
import threading
import time
from collections import OrderedDict, namedtuple

CacheInfo = namedtuple("CacheInfo", "hits misses maxsize currsize")

_KWMARK = object()


def ttl_cache(seconds, maxsize=128, clock=None):
    if isinstance(seconds, bool) or not isinstance(seconds, (int, float)) or seconds <= 0:
        raise ValueError("seconds must be positive")
    if maxsize is not None and (isinstance(maxsize, bool) or not isinstance(maxsize, int) or maxsize <= 0):
        raise ValueError("maxsize must be a positive int or None")
    clock = clock or time.monotonic

    def deco(fn):
        data = OrderedDict()  # key -> (expires, value)
        inflight = {}  # key -> Event
        lock = threading.Lock()
        stats = [0, 0]

        def make_key(args, kwargs):
            key = args
            if kwargs:
                key += (_KWMARK,) + tuple(sorted(kwargs.items()))
            hash(key)
            return key

        @functools.wraps(fn)
        def wrapper(*args, **kwargs):
            key = make_key(args, kwargs)
            while True:
                with lock:
                    now = clock()
                    hit = data.get(key)
                    if hit is not None and now < hit[0]:
                        data.move_to_end(key)
                        stats[0] += 1
                        return hit[1]
                    ev = inflight.get(key)
                    if ev is None:
                        ev = inflight[key] = threading.Event()
                        stats[1] += 1
                        break
                ev.wait()  # then look again: usually a hit now
            try:
                value = fn(*args, **kwargs)
            except BaseException:
                with lock:
                    inflight.pop(key).set()
                raise
            with lock:
                now = clock()
                data.pop(key, None)
                if maxsize is not None and len(data) >= maxsize:
                    for k in [k for k, (exp, _) in data.items() if exp <= now]:
                        del data[k]
                    while len(data) >= maxsize:
                        data.popitem(last=False)
                data[key] = (now + seconds, value)
                inflight.pop(key).set()
            return value

        def cache_info():
            with lock:
                now = clock()
                cur = sum(1 for exp, _ in data.values() if now < exp)
                return CacheInfo(stats[0], stats[1], maxsize, cur)

        def cache_clear():
            with lock:
                data.clear()
                stats[0] = stats[1] = 0

        wrapper.cache_info = cache_info
        wrapper.cache_clear = cache_clear
        return wrapper

    return deco
