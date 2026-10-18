This folder holds the material for the incident of 2026-03-14 in which customers got 503s at checkout: service logs (`logs/`), the deploy log, a commit list with diffs, the incident chat export, and a postmortem draft. `docs/logging.md` documents the log formats.

The incident is the **db-proxy database connection-pool exhaustion** of that evening. Work out the facts for the final postmortem.

Rules for conflicting sources: the logs, `deploys.log` and `commits.txt` are ground truth. The postmortem draft was written in a hurry and contains mistakes; the chat contains guesses that turned out wrong.

Definitions:
- An *incident error* is an ERROR-level log line caused by the db-proxy connection-pool exhaustion (directly or because a dependency failed from it). Errors with other causes don't count, even if they happened at the same time.
- `start_utc` / `end_utc`: timestamps of the first and the last incident error across all services, in UTC.
- `affected_services`: every service that logged at least one incident error (use the service names from the log file names).

Write `ANSWER.json` with exactly this shape:

```json
{
  "root_cause_commit": "<hash of the commit that caused it (at least 7 hex chars)>",
  "start_utc": "<YYYY-MM-DDTHH:MM:SSZ>",
  "end_utc": "<YYYY-MM-DDTHH:MM:SSZ>",
  "affected_services": ["<service>", "..."]
}
```

Don't change any other file.
