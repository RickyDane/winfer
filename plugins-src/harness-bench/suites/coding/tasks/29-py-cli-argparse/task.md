todo.py parses `sys.argv` by hand. Rewrite its command-line handling with `argparse` (subcommands `add`, `list`, `done`, `rm`), keeping everything that works today working exactly the same (same stdout/stderr output, files and exit codes for valid commands, `TODO_FILE` env var, `--file PATH` before the subcommand). In addition:
- `todo.py --version` prints `todo 1.4.0` to stdout and exits 0.
- `todo.py --help` and `todo.py <subcommand> --help` print usage to stdout and exit 0.
- Invalid usage (no subcommand, unknown subcommand, missing or extra arguments, non-integer ID, priority outside 1–5, unknown options) prints an error to stderr and exits with code 2, as argparse does, without touching the todo file.
- `add` takes the text as one or more words and an optional `--priority N` (default 3) that may appear before or after the words; `list` takes `--all` and `--priority N`.
Don't index `sys.argv` yourself anymore. `python3 -m unittest discover -s tests` must still pass.
