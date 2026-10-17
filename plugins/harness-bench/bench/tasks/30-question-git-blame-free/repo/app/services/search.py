"""Pushes documents to the search index in batches, with its own retry backoff."""

from app.net.retry import Backoff


class SearchIndexer:
    def __init__(self, http, batch_size=200, backoff_ms=750):
        self.http = http
        self.batch_size = batch_size
        self.retry = Backoff(backoff_ms, attempts=3)

    def index(self, docs):
        for i in range(0, len(docs), self.batch_size):
            batch = docs[i : i + self.batch_size]
            self.retry.run(lambda: self.http.post("search/documents", batch))
        return len(docs)
