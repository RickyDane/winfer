import sys
import threading
import time
import unittest

from downloader import DownloadQueue

sys.setswitchinterval(1e-6)


def make_fetch(delay=0.0):
    active = [0, 0]
    lock = threading.Lock()

    def fetch(url, attempt):
        with lock:
            active[0] += 1
            active[1] = max(active[1], active[0])
        try:
            if delay:
                time.sleep(delay)
            n = int(url.rsplit("/", 1)[1])
            if n % 5 == 0 and attempt < 2:
                raise IOError("flaky")
            if n % 7 == 0:
                raise IOError("gone")
            return n
        finally:
            with lock:
                active[0] -= 1

    return fetch, active


def expected(urls, max_retries=2):
    done = failed = 0
    by_host, attempts = {}, {}
    for u in urls:
        n = int(u.rsplit("/", 1)[1])
        host = u.split("/")[2]
        ok_at = None
        for a in range(max_retries + 1):
            if not (n % 5 == 0 and a < 2) and not n % 7 == 0:
                ok_at = a
                break
        if ok_at is None:
            failed += 1
            attempts[u] = max_retries + 1
        else:
            done += 1
            attempts[u] = ok_at + 1
            by_host[host] = by_host.get(host, 0) + n
    return {"completed": done, "failed": failed, "bytes_by_host": by_host, "attempts": attempts}


class Stress(unittest.TestCase):
    def run_round(self, workers, n, submitters):
        fetch, _ = make_fetch()
        q = DownloadQueue(fetch, workers=workers, max_retries=2)
        urls = [f"http://h{i % 3}.test/{i}" for i in range(1, n + 1)]
        chunks = [urls[i::submitters] for i in range(submitters)]
        ts = [threading.Thread(target=lambda c=c: [q.submit(u) for u in c]) for c in chunks]
        for t in ts:
            t.start()
        for t in ts:
            t.join()
        q.wait()
        s = q.stats()
        q.close()
        self.assertEqual(s, expected(urls))

    def test_many_rounds(self):
        for r in range(12):
            self.run_round(workers=8 + r % 5, n=400, submitters=1 + r % 4)

    def test_downloads_run_in_parallel(self):
        fetch, active = make_fetch(delay=0.02)
        q = DownloadQueue(fetch, workers=8)
        urls = [f"http://p.test/{i}" for i in range(1, 81) if i % 5 and i % 7]
        start = time.time()
        for u in urls:
            q.submit(u)
        q.wait()
        elapsed = time.time() - start
        s = q.stats()
        q.close()
        self.assertEqual(s["completed"], len(urls))
        self.assertGreaterEqual(active[1], 6, "fetches are serialized")
        self.assertLess(elapsed, len(urls) * 0.02 / 3, "fetches are serialized")


if __name__ == "__main__":
    unittest.main()
