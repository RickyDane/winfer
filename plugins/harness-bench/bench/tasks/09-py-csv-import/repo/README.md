# importer

`importer.csvimport` turns CSV exports into typed rows.

## API

- `parse(text: str, schema: dict[str, str]) -> list[dict]`
- `load(path: str, schema: dict[str, str]) -> list[dict]` reads the file as UTF-8 and calls `parse`.
- `CsvImportError(Exception)` with an integer attribute `line`.

`schema` maps column names to a type: `"str"`, `"int"`, `"float"`, `"bool"` or `"date"`.

## Rules

1. Fields are separated by `,`. A field may be quoted with `"`; inside quotes, `""` is a
   literal `"`, and commas and line breaks are part of the value. Records end with `\n`
   or `\r\n`; a `\r\n` inside a quoted value is kept as `\n`.
2. A leading UTF-8 byte order mark (`﻿`) is ignored, both by `load` and by `parse`.
3. Blank lines (empty, outside quotes) are skipped anywhere in the file.
4. The first non-blank record is the header. Header names are stripped of surrounding
   whitespace. Columns not in `schema` are dropped from the output. If a schema column is
   missing from the header, raise `CsvImportError` with `line` = the header's line.
5. Every data record must have exactly as many fields as the header, otherwise raise
   `CsvImportError` (`line` = the line the record starts on).
6. Each output row is a dict with exactly the schema's keys. Conversion:
   - `str`: the value unchanged (no stripping); empty stays `""`.
   - For every other type, the value is stripped first, and an empty value becomes `None`.
   - `int`: optional sign and digits, e.g. `-12`, `+3`. `"1.5"` or `"1e3"` is an error.
   - `float`: anything Python's `float()` accepts except `nan` and `inf` variants.
   - `bool`: `true/yes/1` -> `True`, `false/no/0` -> `False`, case-insensitive.
   - `date`: `YYYY-MM-DD` -> `datetime.date`; anything else (including invalid dates) is an error.
   A value that can't be converted raises `CsvImportError` whose `line` is the line the
   record starts on and whose message contains the column name.

Line numbers are 1-based physical lines of the input (a quoted value spanning three lines
moves the next record's line number by three).
