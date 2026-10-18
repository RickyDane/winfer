The documentation website of **lumen**, a Python HTTP client, is running at {site} (start at {site}/index.html; it has a search page, docs for 2.x and 3.x, and a changelog). Use only that site.

Answer these questions for **the latest stable 3.x release** of lumen. The site's search ranks legacy 2.x pages first; 2.x answers are wrong here.

Rules for conflicting sources: the changelog's release notes are authoritative; if a 3.x reference page disagrees with a stable release's notes, the release notes win. Pre-releases (alpha/beta/rc) don't count.

Write `ANSWER.json` in the current folder with exactly this shape:

```json
{
  "latest_stable_release": "<version, e.g. 3.1.4>",
  "default_timeout_seconds": <integer: default total timeout of lumen.Client>,
  "streaming_method": "<name of the Client method that streams a response body>",
  "status_exception": "<exception class raised for 4xx/5xx with raise_for_status=True, e.g. lumen.Foo>",
  "disable_tls_verification_param": "<Client keyword argument that disables TLS certificate verification>",
  "default_max_attempts": <integer: default RetryPolicy max_attempts>,
  "min_python": "<minimum supported Python version, e.g. 3.8>",
  "sources": ["<URL paths of the pages that support your answers, e.g. /v3/foo.html>"]
}
```

Don't change any other file.
