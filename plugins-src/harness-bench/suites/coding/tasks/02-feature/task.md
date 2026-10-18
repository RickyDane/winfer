Extend the todo CLI in todo.py:

1. `add` takes an optional `--priority` of `low`, `med` or `high` (default `med`). `list` shows it as `[high]`, `[med]` or `[low]` after the id.
2. `list --sort priority` lists high first, then med, then low; ties keep insertion order. Without `--sort`, the order is unchanged.
3. A new `clear-done` command removes completed todos and prints `Removed N`.

Todos saved before this change (no priority) must load as `med`. Keep the existing output format otherwise.
