import unittest

from downloader import DownloadQueue


def fetch(url, attempt):
    n = int(url.rsplit("/", 1)[1])
    if n % 4 == 0 and attempt == 0:
        raise IOError("flaky")
    if n % 9 == 0:
        raise IOError("gone")
    return n


class Basic(unittest.TestCase):
    def test_single_worker(self):
        q = DownloadQueue(fetch, workers=1)
        for i in range(1, 21):
            q.submit(f"http://h{i % 2}.test/{i}")
        q.wait()
        s = q.stats()
        q.close()
        self.assertEqual(s["completed"], 18)
        self.assertEqual(s["failed"], 2)
        self.assertEqual(s["attempts"]["http://h0.test/4"], 2)
        self.assertEqual(s["attempts"]["http://h1.test/9"], 3)


if __name__ == "__main__":
    unittest.main()
