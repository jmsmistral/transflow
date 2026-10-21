"""Explicit, bounded factory exports and replayable callable configuration."""

from __future__ import annotations

import hashlib
import inspect
import json
import math
from types import ModuleType
from typing import Any

from transflow._declaration_values import DeclarationError
from transflow.declarations import SqlTransform, get_declaration

MAX_EXPORTS = 10000


def _value(value: object, depth: int = 0, budget: list[int] | None = None) -> object:
    budget = [10000, 1024 * 1024] if budget is None else budget
    budget[0] -= 1
    if budget[0] < 0:
        raise DeclarationError("Factory configuration exceeds the supported node limit")
    if isinstance(value, str):
        budget[1] -= len(value.encode("utf-8"))
        if budget[1] < 0:
            raise DeclarationError("Factory configuration exceeds 1 MiB")
    # Only immutable values: mutable cells/defaults can change after discovery.
    if depth > 24:
        raise DeclarationError("Factory configuration exceeds the supported depth")
    if value is None or type(value) in (bool, str):
        return value
    if type(value) is int:
        return {"integer": str(value)}
    if type(value) is float and math.isfinite(value):
        return {"float": value.hex()}
    if type(value) is tuple and len(value) <= MAX_EXPORTS:
        return {"tuple": [_value(item, depth + 1, budget) for item in value]}
    raise DeclarationError(
        "Factory closure/default values must be immutable scalars or tuples; "
        "use declared parameters for runtime settings"
    )


def configuration(function: object) -> str:
    if not inspect.isfunction(function):
        return "{}"
    closure = {}
    budget = [10000, 1024 * 1024]
    for name, cell in zip(function.__code__.co_freevars, function.__closure__ or (), strict=True):
        try:
            value = cell.cell_contents
        except ValueError as exc:
            raise DeclarationError("Factory contains an empty closure cell") from exc
        closure[name] = _value(value, budget=budget)
    value = {
        "module": function.__module__,
        "qualname": function.__qualname__,
        "closure": closure,
        "defaults": _value(function.__defaults__, budget=budget),
        "kwdefaults": {
            k: _value(v, budget=budget) for k, v in (function.__kwdefaults__ or {}).items()
        },
    }
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
    if len(encoded) > 1024 * 1024:
        raise DeclarationError("Factory configuration exceeds 1 MiB")
    return encoded


def export_key(function: object) -> str:
    declaration = get_declaration(function)
    if declaration is None:
        raise DeclarationError("TRANSFORMS must contain decorated producers only")
    ref = declaration.output.ref
    # Stable across collection reordering and repeated nested function names.
    identity = ref if isinstance(ref, str) else ref.dataset_id
    return "export_" + hashlib.sha256(identity.encode()).hexdigest()


def exports(module: ModuleType) -> list[tuple[str, Any, bool]]:
    if "TRANSFORMS" in vars(module):
        values = vars(module)["TRANSFORMS"]
        if type(values) not in (list, tuple) or len(values) > MAX_EXPORTS:
            raise DeclarationError(
                "TRANSFORMS must be a flat list or tuple of at most 10000 producers"
            )
        result = []
        keys = set()
        for function in values:
            if not (inspect.isfunction(function) or isinstance(function, SqlTransform)):
                raise DeclarationError("TRANSFORMS must contain decorated producers only")
            key = export_key(function)
            if key in keys:
                raise DeclarationError("TRANSFORMS contains duplicate output producers")
            keys.add(key)
            configuration(function)
            result.append((key, function, True))
        return result
    result = []
    seen = set()
    for name, function in vars(module).items():
        if id(function) in seen or not (
            inspect.isfunction(function) or isinstance(function, SqlTransform)
        ):
            continue
        seen.add(id(function))
        declaration = get_declaration(function)
        if declaration is not None and declaration.module == module.__name__:
            if inspect.isfunction(function) and "<locals>" in function.__qualname__:
                raise DeclarationError("Factory-generated producers must be exported in TRANSFORMS")
            result.append((name, function, False))
    if len(result) > 1:
        raise DeclarationError("A producing module needs TRANSFORMS to export multiple producers")
    return result


def resolve(module: ModuleType, producer: dict[str, Any]) -> Any:
    for key, function, _ in exports(module):
        if key == producer["function"]:
            return function
    raise DeclarationError("Accepted producer is absent from captured module exports")
