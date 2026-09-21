"""Immutable expectation AST constructors. No data, SQL or engine execution.

T059 supplies typed composition and explicit scalar/value operands. Remaining core
operators and their truth/metric semantics are implemented by T060, evaluation by T061.
"""

from __future__ import annotations

import builtins
import json
from collections.abc import Sequence
from dataclasses import dataclass
from typing import Literal as TypingLiteral
from typing import overload

from ._declaration_values import DeclarationError, frozen_json

Comparison = TypingLiteral["gt", "gte", "lt", "lte", "equals", "not_equals"]


def _name(value: str) -> None:
    if (
        not isinstance(value, str)
        or not 0 < len(value) <= 256
        or builtins.any(ord(c) < 32 or ord(c) == 127 for c in value)
    ):
        raise DeclarationError("Expectation names must be nonempty text without controls")


@dataclass(frozen=True, slots=True)
class Literal:
    """Explicit lossless ScalarValue carrier; ordinary objects are never serialized."""

    _json: bytes

    def __init__(self, value: object) -> None:
        object.__setattr__(self, "_json", frozen_json(value, "ScalarValue"))

    def to_wire(self) -> dict[str, object]:
        return {"kind": "literal", "value": json.loads(self._json)}


@dataclass(frozen=True, slots=True)
class ColumnRef:
    name: str

    def __post_init__(self) -> None:
        _name(self.name)

    def to_wire(self) -> dict[str, object]:
        return {"kind": "column", "name": self.name}

    def non_null(self) -> RowPredicate:
        return RowPredicate._from_node({"kind": "non_null", "columns": [self.name]})


@dataclass(frozen=True, slots=True)
class InputAliasRef:
    """Binding identity; validated against the producer's declared aliases later."""

    alias: str

    def __post_init__(self) -> None:
        _name(self.alias)

    def row_count(self) -> ScalarMetric:
        return ScalarMetric(self)


@dataclass(frozen=True, slots=True)
class ScalarMetric:
    source: InputAliasRef | None = None

    def __post_init__(self) -> None:
        if self.source is not None and not isinstance(self.source, InputAliasRef):
            raise DeclarationError("Metric source must be a declared input reference")

    def to_wire(self) -> dict[str, object]:
        return {
            "kind": "row_count",
            "input": None if self.source is None else {"kind": "input", "alias": self.source.alias},
        }


@dataclass(frozen=True, slots=True, init=False)
class Expectation:
    """Validated immutable boolean AST. Use constructors or from_wire to decode."""

    _json: bytes

    def __init__(self) -> None:
        raise DeclarationError("Use typed expectation constructors")

    @classmethod
    def from_wire(cls, value: object) -> Expectation:
        try:
            raw = frozen_json(value, "ExpectationAstV1")
        except DeclarationError as exc:
            raise DeclarationError(
                "Expectation AST version, operator or fields are unsupported"
            ) from exc
        node = json.loads(raw)
        node.pop("ast_version")
        kind = _validate(node, budget=[1000])
        target = RowPredicate if kind == "row" else DatasetExpectation
        if cls not in (Expectation, target):
            raise DeclarationError("Expression has the wrong boolean kind")
        result = object.__new__(target)
        object.__setattr__(result, "_json", raw)
        return result

    def to_wire(self) -> dict[str, object]:
        result: dict[str, object] = json.loads(self._json)
        return result

    def _node(self) -> dict[str, object]:
        result = self.to_wire()
        result.pop("ast_version")
        return result

    def validate_inputs(self, aliases: set[str]) -> None:
        """Reject hidden lookups without accessing any input dataset."""

        def visit(value: object) -> None:
            if isinstance(value, dict):
                if value.get("kind") == "input" and value.get("alias") not in aliases:
                    raise DeclarationError("Expectation references an undeclared input alias")
                for child in value.values():
                    visit(child)
            elif isinstance(value, list):
                for child in value:
                    visit(child)

        visit(self.to_wire())

    def __bool__(self) -> bool:
        raise DeclarationError("An expectation is syntax; use E.all/E.any/E.not_ for composition")


class RowPredicate(Expectation):
    __slots__ = ()

    @classmethod
    def _from_node(cls, node: dict[str, object]) -> RowPredicate:
        result = cls.from_wire({"ast_version": 1, **node})
        if not isinstance(result, RowPredicate):
            raise DeclarationError("Expected a row predicate")
        return result


class DatasetExpectation(Expectation):
    __slots__ = ()

    @classmethod
    def _from_node(cls, node: dict[str, object]) -> DatasetExpectation:
        result = cls.from_wire({"ast_version": 1, **node})
        if not isinstance(result, DatasetExpectation):
            raise DeclarationError("Expected a dataset expectation")
        return result


def _validate(node: dict[str, object], depth: int = 0, *, budget: list[int]) -> str:
    if depth > 32 or budget[0] <= 0:
        raise DeclarationError("Expectation exceeds its depth or node limit")
    budget[0] -= 1
    kind = node["kind"]
    if kind in ("non_null", "primary_key"):
        columns = node["columns"]
        if (
            not isinstance(columns, list)
            or len(set(columns)) != len(columns)
            or (kind == "non_null" and len(columns) != 1)
        ):
            raise DeclarationError("Expectation columns are repeated or have invalid arity")
        return "row" if kind == "non_null" else "dataset"
    if kind in ("row_compare", "metric_compare"):
        left, right = node["left"], node["right"]
        assert isinstance(left, dict) and isinstance(right, dict)
        operands = (left, right)
        if builtins.all(v["kind"] == "literal" for v in operands):
            raise DeclarationError("Comparison requires a column or metric operand")
        if kind == "metric_compare":
            for operand in operands:
                if operand["kind"] == "literal" and operand["value"]["type"] not in (
                    "i8",
                    "i16",
                    "i32",
                    "i64",
                    "u8",
                    "u16",
                    "u32",
                    "u64",
                ):
                    raise DeclarationError("Row-count comparisons require an integer literal")
        elif builtins.any(
            v["kind"] == "literal" and v["value"]["type"] == "null" for v in operands
        ):
            raise DeclarationError("Use explicit null predicates instead of a null comparison")
        return "row" if kind == "row_compare" else "dataset"
    expected = "row" if str(kind).startswith("row_") or kind == "every" else "dataset"
    children = node.get("children", [node.get("child")])
    assert isinstance(children, list)
    for child in children:
        assert isinstance(child, dict)
        if _validate(child, depth + 1, budget=budget) != expected:
            raise DeclarationError("Mixed boolean kinds require explicit E.every promotion")
    return "dataset" if kind == "every" else expected


def col(name: str) -> ColumnRef:
    return ColumnRef(name)


def literal(value: object) -> Literal:
    return Literal(value)


def input(alias: str) -> InputAliasRef:
    return InputAliasRef(alias)


def row_count() -> ScalarMetric:
    return ScalarMetric()


def primary_key(*columns: str) -> DatasetExpectation:
    return DatasetExpectation._from_node({"kind": "primary_key", "columns": list(columns)})


@overload
def compare(left: ColumnRef, op: Comparison, right: ColumnRef | Literal) -> RowPredicate: ...


@overload
def compare(left: Literal, op: Comparison, right: ColumnRef) -> RowPredicate: ...


@overload
def compare(
    left: ScalarMetric, op: Comparison, right: ScalarMetric | Literal
) -> DatasetExpectation: ...


@overload
def compare(left: Literal, op: Comparison, right: ScalarMetric) -> DatasetExpectation: ...


@overload
def compare(
    left: ColumnRef | Literal | ScalarMetric,
    op: Comparison,
    right: ColumnRef | Literal | ScalarMetric,
) -> Expectation: ...


def compare(
    left: ColumnRef | Literal | ScalarMetric,
    op: Comparison,
    right: ColumnRef | Literal | ScalarMetric,
) -> Expectation:
    """Explicit typed comparison foundation; convenience DSL methods belong to T060."""
    if not isinstance(left, (ColumnRef, Literal, ScalarMetric)) or not isinstance(
        right, (ColumnRef, Literal, ScalarMetric)
    ):
        raise DeclarationError("Comparison requires typed operands")
    metric = isinstance(left, ScalarMetric) or isinstance(right, ScalarMetric)
    if metric and (isinstance(left, ColumnRef) or isinstance(right, ColumnRef)):
        raise DeclarationError("Row columns cannot be compared with scalar metrics")
    return Expectation.from_wire(
        {
            "ast_version": 1,
            "kind": "metric_compare" if metric else "row_compare",
            "op": op,
            "left": left.to_wire(),
            "right": right.to_wire(),
        }
    )


def _combine(operator: str, children: Sequence[Expectation]) -> Expectation:
    if not children or builtins.any(not isinstance(c, Expectation) for c in children):
        raise DeclarationError("Boolean composition requires at least one expectation")
    prefix = "row" if isinstance(children[0], RowPredicate) else "dataset"
    return Expectation.from_wire(
        {
            "ast_version": 1,
            "kind": f"{prefix}_{operator}",
            "children": [c._node() for c in children],
        }
    )


@overload
def all(*children: RowPredicate) -> RowPredicate: ...


@overload
def all(first: DatasetExpectation, /, *children: DatasetExpectation) -> DatasetExpectation: ...


@overload
def all(*children: Expectation) -> Expectation: ...


def all(*children: Expectation) -> Expectation:
    return _combine("all", children)


@overload
def any(*children: RowPredicate) -> RowPredicate: ...


@overload
def any(first: DatasetExpectation, /, *children: DatasetExpectation) -> DatasetExpectation: ...


@overload
def any(*children: Expectation) -> Expectation: ...


def any(*children: Expectation) -> Expectation:
    return _combine("any", children)


@overload
def not_(child: RowPredicate) -> RowPredicate: ...


@overload
def not_(child: DatasetExpectation) -> DatasetExpectation: ...


@overload
def not_(child: Expectation) -> Expectation: ...


def not_(child: Expectation) -> Expectation:
    if not isinstance(child, Expectation):
        raise DeclarationError("Negation requires an expectation")
    prefix = "row" if isinstance(child, RowPredicate) else "dataset"
    return Expectation.from_wire(
        {"ast_version": 1, "kind": f"{prefix}_not", "child": child._node()}
    )


def every(child: RowPredicate) -> DatasetExpectation:
    if not isinstance(child, RowPredicate):
        raise DeclarationError("E.every requires a row predicate")
    return DatasetExpectation._from_node({"kind": "every", "child": child._node()})
