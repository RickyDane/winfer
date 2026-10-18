This folder is the documentation corpus of **tidepool**, a caching proxy for package registries: a CHANGELOG, configuration references for each major version under `docs/`, migration guides and blog posts.

Research the configuration option that controls **how long a fetched artifact stays in tidepool's local cache before it is re-validated** (not the package index refresh interval), over its whole history.

Rules for conflicting sources: `CHANGELOG.md` is the authoritative record of what changed in which release. Docs pages, migration guides and blog posts are sometimes stale or wrong; when they disagree with the CHANGELOG, the CHANGELOG wins. "Current" means the latest released version; entries under "Unreleased" don't count.

Write `ANSWER.json` with exactly this shape:

```json
{
  "original_name": "<option name when it was first added, e.g. section.key>",
  "introduced_in": "<release that first added the option, e.g. 1.2.3>",
  "renamed_in": "<release that first renamed it from its original name>",
  "alias_removed_in": "<release in which the original name stopped working>",
  "current_name": "<name in the latest release>",
  "current_default_seconds": <integer default in the latest release>,
  "latest_release": "<latest released version>",
  "sources": ["<relative paths of the files that support your answer>"]
}
```

Don't change any other file.
