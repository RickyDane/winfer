This is a Node project (`package.json`) with its dependencies installed in `node_modules/` (including nested `node_modules` folders). Audit it for copyleft licenses before we ship it to customers.

Rules:
- Only production dependencies count: the packages in the root `package.json` `dependencies`, and, transitively, the `dependencies` of those packages. `devDependencies` (of the root or of any package) don't count, and neither do packages that sit in `node_modules/` without being reachable that way.
- A dependency is resolved the way Node does it: from the requiring package's own nested `node_modules/` first, then from each parent `node_modules/` up to the root.
- A package's license is given by its license file (`LICENSE`, `LICENSE.md`, `COPYING`, ...) if it has one; otherwise by `package.json` (`license`, or the old `licenses` array). If the file and `package.json` disagree, the file wins.
- Copyleft means any GPL, LGPL, AGPL, MPL or EPL license. For an SPDX expression, `A OR B` is copyleft only if every alternative is copyleft; `A AND B` is copyleft if any part is.
- `docs/` contains an outdated audit; don't trust it.

Write `ANSWER.json` with exactly this shape, one entry per copyleft package (name + installed version), in any order:

```json
{
  "copyleft": [
    {
      "name": "<package name>",
      "version": "<installed version>",
      "license": "<its license as SPDX id or expression>",
      "via": ["<every root production dependency through which it is reached>"]
    }
  ]
}
```

Don't change any other file.
