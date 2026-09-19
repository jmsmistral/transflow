"""Small immutable declaration seed; complete AST/DSL validation is T059/T060.

These constructors declare checks. They do not evaluate data or compute AST hashes.
"""

from dataclasses import dataclass
from typing import Literal

from ._declaration_values import DeclarationError


@dataclass(frozen=True, slots=True)
class Expectation:
    kind: Literal["non_null", "primary_key"]
    columns: tuple[str, ...]

    def __post_init__(self) -> None:
        supplied: object = self.columns
        if isinstance(supplied, (str, bytes)):
            raise DeclarationError("Expectation columns must be a sequence of names")
        columns = tuple(self.columns)
        if self.kind not in {"non_null", "primary_key"}:
            raise DeclarationError("Unsupported expectation operation")
        if not columns or any(not isinstance(c, str) or not c or "\0" in c for c in columns):
            raise DeclarationError("Expectation requires nonempty column names")
        if len(set(columns)) != len(columns) or (self.kind == "non_null" and len(columns) != 1):
            raise DeclarationError("Expectation has duplicate or unsupported column arguments")
        object.__setattr__(self, "columns", columns)


@dataclass(frozen=True, slots=True)
class ColumnRef:
    name: str

    def __post_init__(self) -> None:
        if not isinstance(self.name, str) or not self.name or "\0" in self.name:
            raise DeclarationError("Column name must be nonempty text")

    def non_null(self) -> Expectation:
        return Expectation("non_null", (self.name,))


def col(name: str) -> ColumnRef:
    return ColumnRef(name)


def primary_key(*columns: str) -> Expectation:
    return Expectation("primary_key", columns)
