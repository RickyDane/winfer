"""JSON (de)serialization for dataclasses.

    to_json(obj) -> str
    from_json(cls, text) -> instance of cls

Supported field types (and any nesting of them):
  int, float, str, bool, a dataclass, an Enum subclass, datetime,
  list[X], Optional[X] / X | None.

Encoding: a dataclass is a JSON object with one key per field (field name); an Enum is
its `.value`; a datetime is `datetime.isoformat()`; None is null; lists are arrays.

Decoding (`from_json`) builds the dataclass back, recursively, and raises `ValueError`
(with a message naming the offending field) when the JSON doesn't fit the type:
  - a key that isn't a field of the dataclass (unknown keys are rejected),
  - a missing key for a field without a default (fields with a default or
    default_factory may be missing and get their default),
  - a value of the wrong JSON type: e.g. a string for an int, `true` for an int
    (bool is not an int here), a float for an int; an int IS accepted for a float
    field (and becomes a float),
  - null for a field that isn't Optional,
  - a value that isn't one of the Enum's values, or a string that isn't an ISO datetime.
`from_json` also raises ValueError if the text isn't valid JSON or the top level isn't an
object. Field annotations may be strings (`from __future__ import annotations`).
"""


def to_json(obj):
    raise NotImplementedError


def from_json(cls, text):
    raise NotImplementedError
