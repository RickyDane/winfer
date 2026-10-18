from app.settings.registry import register

register("search.batch_size", 200)
register("search.retry.backoff_ms", 750, doc="Delay between failed index batches.")
register("search.index", "documents")
