import threading
import time
import unittest

from ttlcache import CacheInfo, ttl_cache


class Clock:
    def __init__(self):
        self.t = 1000.0

    def __call__(self):
        return self.t


class Basics(unittest.TestCase):
    def setUp(self):
        self.clock = Clock()
        self.calls = []

    def make(self, seconds=10, maxsize=128):
        @ttl_cache(seconds, maxsize, clock=self.clock)
        def f(*a, **kw):
            """doc of f"""
            self.calls.append((a, kw))
            return (a, tuple(sorted(kw.items())), len(self.calls))

        return f

    def test_validation(self):
        for bad in (0, -1):
            with self.assertRaises(ValueError):
                ttl_cache(bad)
        for bad in (0, -3, 1.5):
            with self.assertRaises(ValueError):
                ttl_cache(5, bad)
        ttl_cache(0.5, None)

    def test_wraps(self):
        f = self.make()
        self.assertEqual(f.__name__, "f")
        self.assertEqual(f.__doc__, "doc of f")
        self.assertTrue(callable(f.__wrapped__))

    def test_hits_and_expiry_boundary(self):
        f = self.make(seconds=10)
        r = f(1)
        self.clock.t += 9.999
        self.assertEqual(f(1), r)
        self.assertEqual(f.cache_info(), CacheInfo(1, 1, 128, 1))
        self.clock.t = 1010.0
        r2 = f(1)
        self.assertNotEqual(r2, r)
        self.assertEqual(f.cache_info(), CacheInfo(1, 2, 128, 1))
        self.clock.t += 5
        self.assertEqual(f(1), r2)
        self.assertEqual(len(self.calls), 2)

    def test_currsize_ignores_expired(self):
        f = self.make(seconds=10)
        f(1)
        self.clock.t += 5
        f(2)
        self.assertEqual(f.cache_info().currsize, 2)
        self.clock.t += 5
        self.assertEqual(f.cache_info().currsize, 1)
        self.clock.t += 5
        self.assertEqual(f.cache_info().currsize, 0)

    def test_kwargs_keys(self):
        f = self.make()
        f(1, a=1, b=2)
        f(1, b=2, a=1)
        self.assertEqual(len(self.calls), 1)
        f(1, 2)
        f(1, a=2)
        f(a=1)
        f(1)
        self.assertEqual(len(self.calls), 5)
        f(1.0)
        self.assertEqual(len(self.calls), 5)
        f((1, "a"))
        f(1, "a")
        self.assertEqual(len(self.calls), 7)

    def test_kwarg_not_confused_with_positional_tuple(self):
        f = self.make()
        f(x=1)
        f(("x", 1))
        f(*("x", 1))
        self.assertEqual(len(self.calls), 3)

    def test_unhashable(self):
        f = self.make()
        with self.assertRaises(TypeError):
            f([1])
        with self.assertRaises(TypeError):
            f(a={})
        self.assertEqual(self.calls, [])

    def test_exceptions_not_cached(self):
        n = []

        @ttl_cache(10, clock=self.clock)
        def g(x):
            n.append(x)
            if len(n) == 1:
                raise RuntimeError("first")
            return x * 2

        with self.assertRaises(RuntimeError):
            g(3)
        self.assertEqual(g(3), 6)
        self.assertEqual(g(3), 6)
        self.assertEqual(len(n), 2)

    def test_lru_eviction(self):
        f = self.make(seconds=100, maxsize=2)
        f(1)
        f(2)
        f(1)  # 1 is now most recent
        f(3)  # evicts 2
        self.assertEqual(f.cache_info().currsize, 2)
        n = len(self.calls)
        f(1)
        f(3)
        self.assertEqual(len(self.calls), n)
        f(2)
        self.assertEqual(len(self.calls), n + 1)

    def test_expired_dropped_before_lru(self):
        @ttl_cache(10, 3, clock=self.clock)
        def f(x):
            self.calls.append(x)
            return x

        f("old")
        self.clock.t += 6
        f("a")
        f("b")
        f("old")  # hit; "old" is most recently used but stored at t=1000
        self.clock.t += 5  # "old" expired, a/b still valid
        f("c")  # must drop expired "old", not LRU "a"
        n = len(self.calls)
        f("a")
        f("b")
        f("c")
        self.assertEqual(len(self.calls), n)

    def test_cache_clear(self):
        f = self.make()
        f(1)
        f(1)
        f.cache_clear()
        self.assertEqual(f.cache_info(), CacheInfo(0, 0, 128, 0))
        f(1)
        self.assertEqual(len(self.calls), 2)

    def test_unbounded(self):
        f = self.make(maxsize=None)
        for i in range(500):
            f(i)
        self.assertEqual(f.cache_info(), CacheInfo(0, 500, None, 500))

    def test_default_clock_is_real(self):
        @ttl_cache(0.05)
        def h(x):
            return object()

        a = h(1)
        self.assertIs(h(1), a)
        time.sleep(0.08)
        self.assertIsNot(h(1), a)

    def test_separate_functions_separate_caches(self):
        f = self.make()
        g = self.make()
        f(1)
        g(1)
        self.assertEqual(len(self.calls), 2)
        self.assertEqual(f.cache_info().misses, 1)


class Threads(unittest.TestCase):
    def test_counters_and_size(self):
        @ttl_cache(100, 16)
        def f(x):
            return x

        def work(seed):
            for i in range(2000):
                f((i * 7 + seed) % 40)

        ts = [threading.Thread(target=work, args=(s,)) for s in range(8)]
        for t in ts:
            t.start()
        for t in ts:
            t.join()
        info = f.cache_info()
        self.assertEqual(info.hits + info.misses, 16000)
        self.assertLessEqual(info.currsize, 16)

    def test_same_key_computed_once(self):
        runs = []

        @ttl_cache(100)
        def slow(x):
            runs.append(x)
            time.sleep(0.3)
            return x + 1

        out = []
        ts = [threading.Thread(target=lambda: out.append(slow(1))) for _ in range(6)]
        for t in ts:
            t.start()
        for t in ts:
            t.join()
        self.assertEqual(out, [2] * 6)
        self.assertEqual(runs, [1])
        info = slow.cache_info()
        self.assertEqual((info.hits, info.misses), (5, 1))

    def test_different_keys_run_in_parallel(self):
        @ttl_cache(100)
        def slow(x):
            time.sleep(0.4)
            return x

        ts = [threading.Thread(target=slow, args=(i,)) for i in range(5)]
        start = time.monotonic()
        for t in ts:
            t.start()
        for t in ts:
            t.join()
        self.assertLess(time.monotonic() - start, 1.2, "different keys blocked each other")


if __name__ == "__main__":
    unittest.main()
