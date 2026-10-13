"""Bounded exact numeric predicates used only by the application check compiler."""

from __future__ import annotations

import importlib
import json
import math
import struct
from decimal import Decimal
from fractions import Fraction
from functools import lru_cache
from typing import Any


@lru_cache(maxsize=256)
def _literal(text: str) -> Fraction:
    value = json.loads(text)
    kind = value["type"]
    if kind in {"f32", "f64"}:
        number = float(value["value"])
        if kind == "f32":
            number = struct.unpack("!f", struct.pack("!f", number))[0]
        if not math.isfinite(number):
            raise ValueError("Approximation literals must be finite")
        return Fraction.from_float(number)
    if kind == "decimal":
        return Fraction(Decimal(value["value"]))
    if kind in {"i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"}:
        return Fraction(int(value["value"]))
    raise ValueError("Approximation requires numeric literals")


def _approximately(value: Fraction, target: str, absolute: str, relative: str) -> bool:
    b, abs_tol, rel_tol = _literal(target), _literal(absolute), _literal(relative)
    if abs_tol < 0 or rel_tol < 0:
        raise ValueError("Approximation tolerances must be nonnegative")
    return abs(value - b) <= max(abs_tol, rel_tol * max(abs(value), abs(b)))


def _float(value: float | None, target: str, absolute: str, relative: str) -> bool | None:
    if value is None:
        return None
    if not math.isfinite(value):
        return False
    return _approximately(Fraction.from_float(value), target, absolute, relative)


def _decimal(value: str | None, target: str, absolute: str, relative: str) -> bool | None:
    if value is None:
        return None
    return _approximately(Fraction(Decimal(value)), target, absolute, relative)


def _age(value: str | None, threshold: str) -> bool | None:
    if value is None:
        return None
    return Fraction(int(value)) <= _literal(threshold) * 1_000_000


def scan(db: Any, query: dict[str, Any]) -> list[str]:
    """Consume bounded Arrow batches; only exact aggregate counts leave the helper."""
    from .environment import ARROW_VERSION

    arrow = importlib.import_module("pyarrow")
    if arrow.__version__ != ARROW_VERSION:
        raise ValueError("Exact numeric checks require the qualified PyArrow version")
    plan = query["row_plan"]
    columns = query["row_columns"]

    def prepare(node: dict[str, Any]) -> None:
        kind = node["kind"]
        if kind in {"all", "any"}:
            for child in node["children"]:
                prepare(child)
        elif kind == "not":
            prepare(node["child"])
        else:
            if not 0 <= node["column"] < columns:
                raise ValueError("Numeric row plan column is outside its projection")
            if kind in {"approx_float", "approx_decimal"}:
                for name in ["value", "abs_tol", "rel_tol"]:
                    literal = _literal(json.dumps(node[name]))
                    if name != "value" and literal < 0:
                        raise ValueError("Tolerances must be nonnegative")
            elif kind == "age":
                _literal(json.dumps(node["value"]))

    def evaluate(node: dict[str, Any], values: list[Any]) -> bool | None:
        kind = node["kind"]
        if kind in {"all", "any"}:
            children = [evaluate(child, values) for child in node["children"]]
            if kind == "all":
                return False if False in children else None if None in children else True
            return True if True in children else None if None in children else False
        if kind == "not":
            child = evaluate(node["child"], values)
            return None if child is None else not child
        value = values[node["column"]]
        if kind == "boolean":
            if value is not None and type(value) is not bool:
                raise ValueError("Expected a three-valued boolean column")
            return True if value is True else False if value is False else None
        if kind == "age":
            return _age(value, json.dumps(node["value"]))
        arguments = [json.dumps(node[name]) for name in ["value", "abs_tol", "rel_tol"]]
        return (_float if kind == "approx_float" else _decimal)(value, *arguments)

    prepare(plan)
    parameters = [value["value"] for value in query["parameters"]]
    reader = db.execute(query["sql"], parameters).to_arrow_reader(
        batch_size=max(1, min(8192, 65536 // columns))
    )
    if len(reader.schema) != columns:
        raise ValueError("Numeric projection differs from the compiled plan")
    total = failed = unknown = 0
    for batch in reader:
        for row in range(batch.num_rows):
            truth = evaluate(plan, [column[row].as_py() for column in batch.columns])
            total += 1
            failed += truth is False
            unknown += truth is None
            if total > 2**64 - 1:
                raise ValueError("Exact row counts exceed the supported range")
    return [str(total), str(failed), str(unknown)]
