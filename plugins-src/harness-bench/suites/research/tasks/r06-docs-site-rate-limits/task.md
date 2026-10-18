The documentation website of the **Orbit API** is running at {site} (start at {site}/index.html). Use only that site.

Our account is on the **Team** plan with the **Burst** add-on (no other add-ons). Work out the effective rate limit, in requests per minute, that applies to us today for each of these endpoints:

- `GET /v2/search`
- `POST /v2/exports`
- `GET /v2/projects/{id}/members`
- `POST /v2/reports`
- `PATCH /v2/webhooks/{id}`

Follow the calculation rules the site documents (tiers, plan multipliers, add-ons, caps). Rule for conflicting sources: the changelog is authoritative; where a reference page or the plans page disagrees with a newer changelog entry, the changelog entry wins.

Write `ANSWER.json` in the current folder with exactly this shape (integers):

```json
{
  "limits": {
    "GET /v2/search": 0,
    "POST /v2/exports": 0,
    "GET /v2/projects/{id}/members": 0,
    "POST /v2/reports": 0,
    "PATCH /v2/webhooks/{id}": 0
  },
  "total": 0,
  "sources": ["<URL paths of the pages you relied on, e.g. /plans.html>"]
}
```

`total` is the sum of the five limits. Don't change any other file.
