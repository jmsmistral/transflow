"""T010 immutable binding prototype; not the future CatalogSnapshotV1 wire schema."""

from __future__ import annotations

import hashlib
import json
import keyword
import re
from contextvars import ContextVar
from dataclasses import dataclass
from uuid import UUID


class CatalogContextError(RuntimeError):
    """Catalogue access has no matching explicit snapshot context."""


class CatalogLookupError(AttributeError):
    """The requested path is absent from the captured catalogue."""


@dataclass(frozen=True, slots=True)
class PrototypeSnapshot:
    workspace_id: str
    entries: tuple[tuple[str, str], ...]

    def __post_init__(self) -> None:
        if str(UUID(self.workspace_id)) != self.workspace_id:
            raise ValueError("Workspace identity must be a canonical UUID")
        entries = tuple(sorted(tuple(entry) for entry in self.entries))
        if len(entries) > 10000:
            raise ValueError("Catalogue exceeds the prototype's 10000-entry limit")
        paths: set[str] = set()
        ids: set[str] = set()
        for path, dataset_id in entries:
            if not path or any(
                not re.fullmatch(r"[a-z][a-z0-9_]*", part) or keyword.iskeyword(part)
                for part in path.split("/")
            ):
                raise ValueError("Dataset paths must contain public, non-keyword identifiers")
            if str(UUID(dataset_id)) != dataset_id:
                raise ValueError("Dataset identity must be a canonical UUID")
            if path in paths or dataset_id in ids:
                raise ValueError("Duplicate dataset path or identity")
            paths.add(path)
            ids.add(dataset_id)
        object.__setattr__(self, "entries", entries)

    @property
    def fingerprint(self) -> str:
        raw = json.dumps(
            {"prototype": 1, "workspace": self.workspace_id, "entries": self.entries},
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
        return hashlib.sha256(raw).hexdigest()

    def children(self, prefix: str) -> tuple[str, ...]:
        start = prefix + "/" if prefix else ""
        return tuple(
            sorted(
                {
                    path[len(start) :].split("/")[0]
                    for path, _ in self.entries
                    if path.startswith(start)
                }
            )
        )


@dataclass(frozen=True, slots=True)
class PrototypeRef:
    workspace_id: str
    dataset_id: str
    path: str
    fingerprint: str


class CatalogNode:
    """Type marker with no public members that could collide with child names."""

    __slots__ = ()


@dataclass(frozen=True, slots=True)
class _BoundNode(CatalogNode):
    _snapshot: PrototypeSnapshot | _CapturedSnapshot
    _path: str

    def __getattr__(self, name: str) -> _BoundNode:
        if name not in self._snapshot.children(self._path):
            raise CatalogLookupError(
                f"Catalogue path {self._path + '/' if self._path else ''}{name!s} "
                "is not registered. "
                "Declare a new output using a string; build/catalog sync will register it "
                "when those operations are implemented."
            )
        return _BoundNode(self._snapshot, f"{self._path}/{name}" if self._path else name)

    def __dir__(self) -> list[str]:
        return list(self._snapshot.children(self._path))


@dataclass(frozen=True, slots=True)
class _CapturedSnapshot:
    """Worker-only adapter for an already validated CatalogSnapshotV1 projection.

    Public wire-snapshot testing/binding APIs and alias expansion remain T030.
    """

    workspace_id: str
    fingerprint: str
    records: tuple[tuple[str, str, str], ...]

    def children(self, prefix: str) -> tuple[str, ...]:
        start = prefix + "/" if prefix else ""
        return tuple(
            sorted(
                {
                    path[len(start) :].split("/")[0]
                    for path, _, _ in self.records
                    if path.startswith(start)
                }
            )
        )


_binding: ContextVar[PrototypeSnapshot | _CapturedSnapshot | None] = ContextVar(
    "transflow_catalog_prototype", default=None
)


@dataclass(frozen=True, slots=True)
class CatalogRoot:
    def __getattr__(self, name: str) -> _BoundNode:
        snapshot = _binding.get()
        if snapshot is None:
            raise CatalogContextError(
                "No catalogue snapshot is bound. Tests must enter catalog_context before "
                "importing catalogue-dependent modules; worker binding is not implemented yet."
            )
        return _BoundNode(snapshot, "").__getattr__(name)

    def __dir__(self) -> list[str]:
        snapshot = _binding.get()
        return list(snapshot.children("")) if snapshot is not None else []


def resolve_reference(node: CatalogNode, *, expected_fingerprint: str) -> PrototypeRef:
    """Resolve a captured node; this does not resolve a dataset version or branch head."""
    if not isinstance(node, _BoundNode):
        raise TypeError("Expected a bound catalogue node")
    snapshot = node._snapshot
    if snapshot.fingerprint != expected_fingerprint:
        raise CatalogContextError("Catalogue fingerprint is stale or belongs to another workspace")
    if isinstance(snapshot, _CapturedSnapshot):
        for path, owner, dataset_id in snapshot.records:
            if path == node._path:
                return PrototypeRef(owner, dataset_id, path, snapshot.fingerprint)
        raise CatalogContextError("This catalogue node is a namespace, not a registered dataset")
    for path, dataset_id in snapshot.entries:
        if path == node._path:
            return PrototypeRef(snapshot.workspace_id, dataset_id, path, snapshot.fingerprint)
    raise CatalogContextError("This catalogue node is a namespace, not a registered dataset")


def capture_reference(node: CatalogNode) -> PrototypeRef:
    """Capture the node's own immutable identity without consulting ambient context."""
    if not isinstance(node, _BoundNode):
        raise TypeError("Expected a bound catalogue node")
    return resolve_reference(node, expected_fingerprint=node._snapshot.fingerprint)
