"""Typed CSV import. See README.md for the spec."""

import datetime
import math
import re


class CsvImportError(Exception):
    def __init__(self, message, line):
        super().__init__(f"line {line}: {message}")
        self.line = line


def _records(text):
    """Yields (start line, fields) per record; blank lines yield nothing."""
    i, n, line = 0, len(text), 1
    while i < n:
        start = line
        fields, cur, quoted_any = [], [], False
        while True:
            if i >= n:
                fields.append("".join(cur))
                break
            ch = text[i]
            if ch == '"' and not cur and not quoted_any:
                quoted_any = True
                i += 1
                while True:
                    if i >= n:
                        raise CsvImportError("unterminated quote", start)
                    ch = text[i]
                    if ch == '"':
                        if i + 1 < n and text[i + 1] == '"':
                            cur.append('"')
                            i += 2
                            continue
                        i += 1
                        break
                    if ch == "\r" and i + 1 < n and text[i + 1] == "\n":
                        cur.append("\n")
                        line += 1
                        i += 2
                        continue
                    if ch == "\n":
                        line += 1
                    cur.append(ch)
                    i += 1
                continue
            if ch == ",":
                fields.append("".join(cur))
                cur, quoted_any = [], False
                i += 1
                continue
            if ch == "\n" or (ch == "\r" and i + 1 < n and text[i + 1] == "\n"):
                i += 2 if ch == "\r" else 1
                line += 1
                fields.append("".join(cur))
                break
            cur.append(ch)
            i += 1
        if fields == [""] and not quoted_any:
            continue
        yield start, fields


_INT = re.compile(r"[+-]?\d+\Z")
_DATE = re.compile(r"\d{4}-\d{2}-\d{2}\Z")
_BOOL = {"true": True, "yes": True, "1": True, "false": False, "no": False, "0": False}


def _convert(kind, raw):
    if kind == "str":
        return raw
    v = raw.strip()
    if v == "":
        return None
    if kind == "int":
        if not _INT.match(v):
            raise ValueError
        return int(v)
    if kind == "float":
        f = float(v)
        if math.isnan(f) or math.isinf(f):
            raise ValueError
        return f
    if kind == "bool":
        return _BOOL[v.lower()]
    if kind == "date":
        if not _DATE.match(v):
            raise ValueError
        return datetime.date.fromisoformat(v)
    raise ValueError(f"unknown type {kind}")


def parse(text, schema):
    if text.startswith("﻿"):
        text = text[1:]
    rows, header, index = [], None, {}
    for line, fields in _records(text):
        if header is None:
            header = [f.strip() for f in fields]
            for col in schema:
                if col not in header:
                    raise CsvImportError(f"missing column {col}", line)
            index = {col: header.index(col) for col in schema}
            continue
        if len(fields) != len(header):
            raise CsvImportError(f"expected {len(header)} fields, got {len(fields)}", line)
        row = {}
        for col, kind in schema.items():
            try:
                row[col] = _convert(kind, fields[index[col]])
            except (ValueError, KeyError):
                raise CsvImportError(f"bad {kind} in column {col}: {fields[index[col]]!r}", line) from None
        rows.append(row)
    return rows


def load(path, schema):
    with open(path, encoding="utf-8", newline="") as f:
        return parse(f.read(), schema)
