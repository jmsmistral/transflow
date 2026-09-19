"""Immutable CatalogSnapshotV1 binding. Lookup performs no I/O or producer imports."""

from __future__ import annotations

import json
from collections.abc import Mapping
from contextvars import ContextVar
from dataclasses import dataclass
from threading import Lock
from types import MappingProxyType
from typing import Any


class CatalogContextError(RuntimeError):
    """Catalogue access has no matching explicit immutable snapshot context."""


class CatalogLookupError(AttributeError):
    """The requested path is absent from the captured catalogue."""


@dataclass(frozen=True, slots=True)
class DatasetRef:
    """Owner-qualified identity and captured name; this never selects a data version."""

    workspace_id: str
    dataset_id: str
    path: str
    fingerprint: str


@dataclass(frozen=True, slots=True, init=False)
class CatalogSnapshot:
    """A verified private copy of CatalogSnapshotV1, including explicit alias names.

    Construct from a decoded JSON document supplied by the coordinator or a test.
    The embedded fingerprint must match its contents. No implicit workspace search
    or operational database access occurs here.
    """

    workspace_id: str
    source_snapshot_id: str
    fingerprint: str
    _document: bytes
    _references: Mapping[str, DatasetRef]
    _children: Mapping[str, tuple[str, ...]]

    def __init__(self, document: object) -> None:
        from transflow_worker.canonical import catalog_fingerprint
        from transflow_worker.wire import validate_document

        try:
            raw = json.dumps(document, allow_nan=False, ensure_ascii=False, sort_keys=True).encode()
            if len(raw) > 16 * 1024 * 1024:
                raise ValueError("Catalogue snapshot exceeds 16 MiB")
            value: dict[str, Any] = json.loads(raw)
            validate_document("CatalogSnapshotV1", value)
            if catalog_fingerprint(value).hex != value["catalog_fingerprint"]:
                raise ValueError("Catalogue fingerprint mismatch")
            records = [*value["entries"], *value.get("aliases", [])]
            if len(records) > 100_000:
                raise ValueError("Catalogue snapshot exceeds 100000 names")
            if any(len(e["path"]) > 4096 or e["path"].count("/") >= 64 for e in records):
                raise ValueError("Catalogue name exceeds 4096 characters or 64 segments")
        except (TypeError, ValueError, RecursionError) as exc:
            raise CatalogContextError(
                "Catalogue snapshot is invalid, stale or incompatible"
            ) from exc
        fingerprint = value["catalog_fingerprint"]
        refs = {
            entry["path"]: DatasetRef(
                entry["key"]["workspace_id"], entry["key"]["dataset_id"], entry["path"], fingerprint
            )
            for entry in records
        }
        children: dict[str, set[str]] = {"": set()}
        for path in refs:
            parts = path.split("/")
            for index, part in enumerate(parts):
                prefix = "/".join(parts[:index])
                children.setdefault(prefix, set()).add(part)
            children.setdefault(path, set())
        object.__setattr__(self, "workspace_id", value["workspace_id"])
        object.__setattr__(self, "source_snapshot_id", value["source_snapshot_id"])
        object.__setattr__(self, "fingerprint", fingerprint)
        object.__setattr__(self, "_document", raw)
        object.__setattr__(self, "_references", MappingProxyType(refs))
        object.__setattr__(
            self, "_children", MappingProxyType({k: tuple(sorted(v)) for k, v in children.items()})
        )

    def document(self) -> dict[str, Any]:
        """Return a detached wire copy; mutating it cannot alter bound references."""
        value: dict[str, Any] = json.loads(self._document)
        return value

    @property
    def paths(self) -> tuple[str, ...]:
        """All canonical and explicitly aliased dataset paths, in lexical order."""
        return tuple(sorted(self._references))

    def children(self, prefix: str) -> tuple[str, ...]:
        """Direct namespace children, without opening files or resolving versions."""
        return self._children.get(prefix, ())


class CatalogNode:
    """Type marker with no public metadata members competing with child names."""

    __slots__ = ()


@dataclass(frozen=True, slots=True)
class _BoundNode(CatalogNode):
    _snapshot: CatalogSnapshot
    _path: str

    def __getattr__(self, name: str) -> _BoundNode:
        if name not in self._snapshot.children(self._path):
            raise CatalogLookupError(
                f"Catalogue path {self._path + '/' if self._path else ''}{name!s} "
                "is not registered. Declare a new output using a string; build/catalog sync "
                "will register it when those commands are available."
            )
        return _BoundNode(self._snapshot, f"{self._path}/{name}" if self._path else name)

    def __dir__(self) -> list[str]:
        return list(self._snapshot.children(self._path))


_test_binding: ContextVar[CatalogSnapshot | None] = ContextVar(
    "transflow_test_catalog", default=None
)
_worker_snapshot: CatalogSnapshot | None = None
_worker_lock = Lock()


def _bind_worker(snapshot: CatalogSnapshot, *, expected_fingerprint: str) -> None:
    """Install exactly one immutable binding for this fresh worker's entire lifetime."""
    global _worker_snapshot
    if snapshot.fingerprint != expected_fingerprint:
        raise CatalogContextError("Cannot bind a stale or mismatched catalogue fingerprint")
    with _worker_lock:
        if _worker_snapshot is not None:
            raise CatalogContextError("The worker already owns its catalogue snapshot")
        if _test_binding.get() is not None:
            raise CatalogContextError("A test context cannot become a worker binding")
        _worker_snapshot = snapshot


def _active_snapshot() -> CatalogSnapshot | None:
    return _worker_snapshot if _worker_snapshot is not None else _test_binding.get()


def _require_testing_context() -> None:
    if _worker_snapshot is not None:
        raise CatalogContextError("A worker's catalogue cannot be replaced by a test context")


@dataclass(frozen=True, slots=True)
class CatalogRoot:
    def __getattr__(self, name: str) -> _BoundNode:
        snapshot = _active_snapshot()
        if snapshot is None:
            raise CatalogContextError(
                "No catalogue snapshot is bound. Tests must enter "
                "transflow.testing.catalog_context "
                "before importing catalogue-dependent modules."
            )
        return _BoundNode(snapshot, "").__getattr__(name)

    def __dir__(self) -> list[str]:
        snapshot = _active_snapshot()
        return list(snapshot.children("")) if snapshot is not None else []


def resolve_reference(node: CatalogNode, *, expected_fingerprint: str) -> DatasetRef:
    """Check a node's captured fingerprint and return identity, without branch/version lookup."""
    if not isinstance(node, _BoundNode):
        raise TypeError("Expected a bound catalogue node")
    if node._snapshot.fingerprint != expected_fingerprint:
        raise CatalogContextError("Catalogue fingerprint is stale or belongs to another workspace")
    reference = node._snapshot._references.get(node._path)
    if reference is None:
        raise CatalogContextError("This catalogue node is a namespace, not a registered dataset")
    return reference


def capture_reference(node: CatalogNode) -> DatasetRef:
    """Capture the node's own identity without consulting a later ambient context."""
    if not isinstance(node, _BoundNode):
        raise TypeError("Expected a bound catalogue node")
    return resolve_reference(node, expected_fingerprint=node._snapshot.fingerprint)
