# tiny-json-patch

`applyPatch(doc, patch)` applies a JSON Patch (RFC 6902) to a JSON document and returns the
result. `doc` is any JSON value (object, array, string, number, boolean, null); `patch` is an
array of operation objects.

- Operations: `add`, `remove`, `replace`, `move`, `copy`, `test`, with the semantics of RFC 6902.
- Paths and `from` are JSON Pointers (RFC 6901): `""` is the whole document, otherwise
  `/`-separated reference tokens where `~1` means `/` and `~0` means `~` (so `/a~1b` is key
  `a/b` and `/m~0n` is key `m~n`). A non-empty pointer that doesn't start with `/` is an error.
- Array indexes are decimal without leading zeros (`0`, `10`; not `01`, `-1`, `1e0`). In `add`,
  the index may equal the array length, and `-` means "append"; elsewhere an index must exist.
- `add` on an existing object member replaces it; `add` on an array index inserts.
- `replace` and `remove` require the target to exist. `move` must not move a value into one of
  its own children. `copy` and `move` with the same `from` and `path` are allowed.
- `test` compares with JSON deep equality: object key order doesn't matter, array order does,
  and `1` is not `"1"`.
- Any problem (unknown `op`, missing `path`/`value`/`from`, bad pointer, missing target,
  failed `test`, ...) throws a `PatchError` (exported from `src/patch.js`).
- Atomic and pure: the input `doc` is never modified, neither on success nor when an operation
  fails part-way, and the result shares no objects or arrays with `doc` or with the `value`s in
  the patch.
