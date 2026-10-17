# async-helpers

## `mapLimit(items, limit, fn) -> Promise<results[]>`

Calls `fn(item, index)` (which returns a promise) for every item, with at most `limit`
calls in flight at any time. Resolves with the results **in input order** (not completion
order). An empty `items` resolves to `[]`.

If any call rejects, the returned promise rejects with that error and no further calls
are started (calls already in flight are left to finish, their results are ignored).
No unhandled rejections may escape.

## `retry(fn, { retries = 3, delayMs = 0 } = {}) -> Promise<value>`

Calls `fn(attempt)` (attempt starts at 1) and resolves with its result. If it rejects,
waits `delayMs` and tries again, up to `retries` more times (so at most `retries + 1`
calls). If every attempt fails, rejects with the error of the **last** attempt.
