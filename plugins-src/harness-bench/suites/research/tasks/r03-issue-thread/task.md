This folder is an export of the issue tracker of **skiff**, an object-storage CLI: issues in `export/issues/`, pull requests in `export/pulls/`, releases in `export/releases.json`, plus a few docs pages.

Reconstruct the history of bug **#412** (corrupted large multipart uploads).

Rules for conflicting sources: `export/releases.json` is the ground truth for which pull request shipped in which release. Issue comments, PR descriptions and docs pages can be wrong or outdated. A pull request that was closed without merging, or merged and later reverted, did not fix the bug.

Write `ANSWER.json` with exactly this shape:

```json
{
  "introduced_by_pr": <number of the PR that introduced the bug>,
  "first_affected_release": "<first release that contained the bug, e.g. 1.2.3>",
  "final_fix_pr": <number of the PR on the main branch that actually fixed the bug for good>,
  "first_fixed_release": "<earliest release, by publish date, that shipped the final fix or a backport of it>",
  "backport_pr": <number of the merged PR that backported the fix to an older release branch>,
  "backport_release": "<release that shipped that backport>",
  "workaround": {"setting": "<config key the maintainers recommend for affected versions>", "value": <recommended value>},
  "sources": ["<relative paths of the files that support your answer>"]
}
```

Don't change any other file.
