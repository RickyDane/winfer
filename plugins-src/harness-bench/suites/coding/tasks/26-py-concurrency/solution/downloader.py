"""A download-queue simulator: worker threads take URLs from a queue and call `fetch`.

`fetch(url, attempt)` returns the number of bytes downloaded or raises an exception. Failed
downloads are retried up to `max_retries` times, then counted as failed.
"""

import collections
import threading
import time
from urllib.parse import urlparse


def _yield():
    # Simulates the bookkeeping I/O (progress callbacks, logging) between steps.
    time.sleep(0)


class DownloadQueue:
    def __init__(self, fetch, workers=4, max_retries=2):
        self.fetch = fetch
        self.max_retries = max_retries
        self.completed = 0
        self.failed = 0
        self.bytes_by_host = {}
        self.attempts = {}
        self._pending = collections.deque()
        self._inflight = 0
        self._queue_lock = threading.Lock()
        self._stats_lock = threading.Lock()
        self._has_work = threading.Condition(self._queue_lock)
        self._idle = threading.Condition(self._queue_lock)
        self._closed = False
        self._threads = [threading.Thread(target=self._worker, daemon=True) for _ in range(workers)]
        for t in self._threads:
            t.start()

    def submit(self, url):
        with self._queue_lock:
            if self._closed:
                raise RuntimeError("queue is closed")
            with self._stats_lock:
                self.attempts.setdefault(url, 0)
            self._pending.append(url)
            self._inflight += 1
            _yield()
            self._has_work.notify()

    def wait(self):
        """Blocks until every submitted URL has completed or failed."""
        with self._queue_lock:
            while self._inflight:
                self._idle.wait()

    def close(self):
        with self._queue_lock:
            self._closed = True
            self._has_work.notify_all()
        for t in self._threads:
            t.join()

    def stats(self):
        with self._stats_lock:
            return {
                "completed": self.completed,
                "failed": self.failed,
                "bytes_by_host": dict(self.bytes_by_host),
                "attempts": dict(self.attempts),
            }

    def _worker(self):
        while True:
            with self._queue_lock:
                while not self._pending and not self._closed:
                    self._has_work.wait()
                if not self._pending:
                    return
                url = self._pending.popleft()
            try:
                size = self.fetch(url, self.attempts[url])
            except Exception:
                self._retry_or_fail(url)
            else:
                self._record(url, size)
            self._done_one()

    def _record(self, url, size):
        host = urlparse(url).netloc
        with self._stats_lock:
            self.attempts[url] += 1
            done = self.completed
            _yield()
            self.completed = done + 1
            total = self.bytes_by_host.get(host, 0)
            _yield()
            self.bytes_by_host[host] = total + size

    def _retry_or_fail(self, url):
        with self._stats_lock:
            self.attempts[url] += 1
            retry = self.attempts[url] <= self.max_retries
            if not retry:
                self.failed += 1
        if retry:
            # Re-queue it; counts as a new in-flight item until it finishes. Never take the
            # queue lock while holding the stats lock (submit takes them the other way round).
            with self._queue_lock:
                self._pending.append(url)
                self._inflight += 1
                self._has_work.notify()

    def _done_one(self):
        with self._queue_lock:
            self._inflight -= 1
            if not self._inflight:
                self._idle.notify_all()
