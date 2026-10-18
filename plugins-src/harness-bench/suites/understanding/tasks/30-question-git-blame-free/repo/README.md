# sync

Pulls data from upstream APIs and pushes change events. Configuration keys are registered in
`app/settings/defaults/`; each can be overridden with an environment variable
(`SYNC_<KEY>` with dots as underscores) or programmatically via `build(overrides)`.

Run `python3 -m app.main` to print the effective HTTP retry delays.
