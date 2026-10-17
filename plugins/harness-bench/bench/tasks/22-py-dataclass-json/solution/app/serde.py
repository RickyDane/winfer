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


import dataclasses
import enum
import json
import types
import typing
from datetime import datetime

_MISSING = dataclasses.MISSING


def _encode(v):
    if dataclasses.is_dataclass(v) and not isinstance(v, type):
        return {f.name: _encode(getattr(v, f.name)) for f in dataclasses.fields(v)}
    if isinstance(v, enum.Enum):
        return _encode(v.value)
    if isinstance(v, datetime):
        return v.isoformat()
    if isinstance(v, (list, tuple)):
        return [_encode(x) for x in v]
    return v


def to_json(obj):
    return json.dumps(_encode(obj))


def _optional_inner(tp):
    origin = typing.get_origin(tp)
    if origin is typing.Union or origin is types.UnionType:
        args = typing.get_args(tp)
        if type(None) in args:
            rest = [a for a in args if a is not type(None)]
            if len(rest) == 1:
                return True, rest[0]
    return False, tp


def _decode(tp, v, where):
    is_opt, tp = _optional_inner(tp)
    if v is None:
        if is_opt:
            return None
        raise ValueError(f"{where}: null not allowed")
    if dataclasses.is_dataclass(tp):
        return _decode_dc(tp, v, where)
    if typing.get_origin(tp) is list:
        if not isinstance(v, list):
            raise ValueError(f"{where}: expected a list")
        (inner,) = typing.get_args(tp) or (typing.Any,)
        return [_decode(inner, x, f"{where}[{i}]") for i, x in enumerate(v)]
    if isinstance(tp, type) and issubclass(tp, enum.Enum):
        try:
            return tp(v)
        except (ValueError, TypeError):
            raise ValueError(f"{where}: {v!r} is not a valid {tp.__name__}") from None
    if tp is datetime:
        if not isinstance(v, str):
            raise ValueError(f"{where}: expected an ISO datetime string")
        try:
            return datetime.fromisoformat(v)
        except ValueError:
            raise ValueError(f"{where}: bad datetime {v!r}") from None
    if tp is bool:
        if not isinstance(v, bool):
            raise ValueError(f"{where}: expected a bool")
        return v
    if tp is int:
        if isinstance(v, bool) or not isinstance(v, int):
            raise ValueError(f"{where}: expected an int")
        return v
    if tp is float:
        if isinstance(v, bool) or not isinstance(v, (int, float)):
            raise ValueError(f"{where}: expected a number")
        return float(v)
    if tp is str:
        if not isinstance(v, str):
            raise ValueError(f"{where}: expected a string")
        return v
    if tp is typing.Any:
        return v
    raise TypeError(f"{where}: unsupported type {tp!r}")


def _decode_dc(cls, data, where):
    if not isinstance(data, dict):
        raise ValueError(f"{where}: expected an object for {cls.__name__}")
    hints = typing.get_type_hints(cls)
    fields = {f.name: f for f in dataclasses.fields(cls) if f.init}
    unknown = set(data) - set(fields)
    if unknown:
        raise ValueError(f"{where}: unknown field(s) {sorted(unknown)} for {cls.__name__}")
    kwargs = {}
    for name, f in fields.items():
        path = f"{where}.{name}" if where else name
        if name not in data:
            if f.default is _MISSING and f.default_factory is _MISSING:
                raise ValueError(f"{path}: missing")
            continue
        kwargs[name] = _decode(hints[name], data[name], path)
    return cls(**kwargs)


def from_json(cls, text):
    try:
        data = json.loads(text)
    except json.JSONDecodeError as e:
        raise ValueError(f"invalid JSON: {e}") from None
    if not isinstance(data, dict):
        raise ValueError("top level must be an object")
    return _decode_dc(cls, data, "")
