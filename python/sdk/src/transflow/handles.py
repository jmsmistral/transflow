"""Execution-owned metadata for exact input pins and the declared destination.

Handles never resolve a moving head. Schema values are independent copies of the
LogicalSchemaV1 document (column names, logical types and nullability).
"""

from __future__ import annotations

import json
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any, cast


@dataclass(frozen=True, slots=True)
class OutputDataset:
    """Destination metadata. An unpublished output has no version or prior data."""

    dataset_id: str
    path: str
    alias: str
    branch: str
    _schema_json: bytes | None = field(repr=False)

    @property
    def schema(self) -> dict[str, Any] | None:
        return None if self._schema_json is None else json.loads(self._schema_json)

    @property
    def version(self) -> None:
        return None


@dataclass(frozen=True, slots=True)
class InputDataset:
    """One exact planned binding, including its requested and supplying branches."""

    dataset_id: str
    path: str
    alias: str
    branch: str
    resolved_branch: str
    version: str
    _schema_json: bytes = field(repr=False)
    _dataframe: Callable[[], Any] = field(repr=False, compare=False)

    @property
    def schema(self) -> dict[str, Any]:
        return cast(dict[str, Any], json.loads(self._schema_json))

    def dataframe(self) -> Any:
        """Return a lazy Polars scan of this binding's verified immutable files."""
        return self._dataframe()
