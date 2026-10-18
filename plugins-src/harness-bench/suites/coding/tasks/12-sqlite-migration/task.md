The contacts app stores a single `people.name` column. Split it into `first_name` and `last_name`:

1. Add migration `migrations/002_split_name.sql` (or `002_split_name.py` with `up(conn)`; see contacts/db.py). It must convert existing data: trim the name; `last_name` is the part after the last space, `first_name` everything before it (trimmed); a one-word name becomes `first_name` with `last_name` = `''`. Drop the `name` column and add an index `idx_people_last_first` on `(last_name, first_name)`. Keep ids, emails and created_at unchanged. Don't edit 001.
2. Update contacts/repo.py and contacts/cli.py to the new columns: `add_person(conn, first_name, last_name, email=None)`, `find_by_last_name(conn, last_name)` (exact match, ordered by first_name, id), `list_people(conn)` ordered by last_name, first_name, id, and `display_name(row)` = "First Last" (just "First" if there is no last name). `cli.py add` takes `FIRST LAST [--email E]`.

Apply the migration to data/contacts.db too (`python3 -m contacts.cli migrate`).
