Add overdue tracking to legacy/invoices.py:

- Add a new function `overdueInvoices(invoices, today)` returning the open invoices whose `dueDate` is strictly before `today` (a `datetime.date`), in input order. Paid and void invoices are never overdue.
- `summaryReport` gets an optional `today=None` parameter. When `today` is given, append one more line `Overdue: <count> (<total> EUR)` using the same money formatting as the other lines, e.g. `Overdue: 1 (120.50 EUR)`. Without `today`, the output stays exactly as it is now.

This is legacy code that other services import. Rules: keep the existing style (camelCase names, `%` formatting) and API — no renames, no signature changes other than the new `today` parameter on `summaryReport`, no reformatting, and don't "clean up" or modernise any existing code, even where it looks wrong. Only change what the feature needs, and don't change the tests or any other file.
