We need an embedded key-value store for a Python service. `REQUIREMENTS.md` lists our constraints C1-C6. `libs/` has the documentation of three candidates (stashdb, burrow, kvelt), and `blog/` has some third-party write-ups.

Exactly one candidate satisfies every constraint. Each of the other two fails exactly one constraint. Find them.

Rules for conflicting sources: a library's `CHANGELOG.md` is the authoritative history of its releases, and its `LICENSE` file is authoritative for licensing. READMEs, docs pages, FAQs and blog posts may be outdated; when they disagree with the CHANGELOG or LICENSE, those win. "Current release" means the newest release in the CHANGELOG.

Write `ANSWER.json` with exactly this shape:

```json
{
  "choice": "<name of the library that satisfies all constraints>",
  "failures": {
    "<other library>": "<id of the constraint it fails, e.g. C4>",
    "<other library>": "<id>"
  }
}
```

Don't change any other file.
