"""Production wire snapshots, alias identities and one-time installed worker bindings."""

import builtins
import copy
import importlib
import json
import sqlite3
from concurrent.futures import ThreadPoolExecutor
from dataclasses import FrozenInstanceError
from pathlib import Path
from typing import Any
from uuid import UUID

import pytest
from test_packaging import installed as installed
from test_packaging import run
from test_packaging import wheel as wheel
from transflow import Input, Output
from transflow.catalog import (
    C,
    CatalogContextError,
    CatalogLookupError,
    CatalogSnapshot,
    resolve_reference,
)
from transflow.testing import catalog_context
from transflow_worker.canonical import DigestKind, content_digest


def document(workspace: int = 1, name: str = "orders") -> dict[str, Any]:
    owner, foreign = str(UUID(int=workspace)), str(UUID(int=workspace + 20))
    key = {"workspace_id": owner, "dataset_id": str(UUID(int=100))}
    foreign_key = {"workspace_id": foreign, "dataset_id": str(UUID(int=200))}
    value: dict[str, Any] = {
        "format_version": 1,
        "workspace_id": owner,
        "source_snapshot_id": str(UUID(int=300)),
        "catalog_fingerprint": "0" * 64,
        "entries": [
            {"path": f"raw/{name}", "key": key, "kind": "transform"},
            {"path": "external/provider/items", "key": foreign_key, "kind": "external"},
        ],
        "aliases": [
            {"path": f"raw/{name}/daily", "key": key},
            {"path": "old/items", "key": key},
            {"path": "external/alternate/items", "key": foreign_key},
        ],
    }
    return sign(value)


def sign(value: dict[str, Any]) -> dict[str, Any]:
    projection = {
        k: v for k, v in value.items() if k not in {"source_snapshot_id", "catalog_fingerprint"}
    }
    value["catalog_fingerprint"] = content_digest(DigestKind.CATALOG, projection).hex
    return value


def test_production_snapshot_copies_wire_bytes_and_retains_alias_ownership() -> None:
    wire = document()
    snapshot = CatalogSnapshot(wire)
    original = snapshot.document()
    wire["entries"][0]["path"] = "raw/mutated"
    detached = snapshot.document()
    detached["aliases"].clear()
    assert snapshot.document() == original
    with catalog_context(snapshot, expected_fingerprint=snapshot.fingerprint):
        local = resolve_reference(C.raw.orders, expected_fingerprint=snapshot.fingerprint)
        alias = resolve_reference(C.old.items, expected_fingerprint=snapshot.fingerprint)
        assert (local.workspace_id, local.dataset_id) == (alias.workspace_id, alias.dataset_id)
        assert alias.path == "old/items"
        assert Input(C.raw.orders.daily).ref.dataset_id == local.dataset_id  # type: ignore[union-attr]
        foreign = resolve_reference(
            C.external.alternate.items, expected_fingerprint=snapshot.fingerprint
        )
        assert foreign.workspace_id != snapshot.workspace_id
        with pytest.raises(ValueError, match="read boundaries"):
            Output(C.external.alternate.items)
        for node in [C.raw, C.external, C.old]:
            with pytest.raises(CatalogContextError, match="namespace"):
                Input(node)
        with pytest.raises(CatalogLookupError, match="string"):
            _ = C.raw.mistyped
        with pytest.raises(FrozenInstanceError):
            local.path = "new"  # type: ignore[misc]
        with pytest.raises(FrozenInstanceError):
            C.old._path = "raw/mutated"  # type: ignore[misc]
    with pytest.raises(FrozenInstanceError):
        snapshot.fingerprint = "a" * 64  # type: ignore[misc]


@pytest.mark.parametrize(
    "mutation",
    [
        "stale",
        "foreign",
        "local_alias",
        "unknown_alias",
        "duplicate_alias",
        "namespace",
        "keyword",
        "unknown_field",
        "version",
    ],
)
def test_invalid_context_fails_before_binding(mutation: str) -> None:
    value = document()
    if mutation == "stale":
        value["catalog_fingerprint"] = "b" * 64
    elif mutation == "foreign":
        value["entries"][1]["kind"] = "transform"
    elif mutation == "local_alias":
        value["aliases"][0]["path"] = "external/local"
    elif mutation == "unknown_alias":
        value["aliases"][0]["key"] = {
            "workspace_id": str(UUID(int=1)),
            "dataset_id": str(UUID(int=999)),
        }
    elif mutation == "duplicate_alias":
        value["aliases"].append(copy.deepcopy(value["aliases"][0]))
    elif mutation == "namespace":
        value["entries"][0]["path"] = "raw/"
    elif mutation == "keyword":
        value["aliases"][0]["path"] = "raw/class"
    elif mutation == "unknown_field":
        value["latest"] = "not authority"
    else:
        value["format_version"] = 2
    if mutation != "stale":
        sign(value)
    with pytest.raises(CatalogContextError, match="invalid, stale or incompatible"):
        CatalogSnapshot(value)
    assert dir(C) == []


def test_lookup_and_declaration_capture_do_not_touch_files_db_or_import_producers(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    snapshot = CatalogSnapshot(document())

    def forbidden(*args: object, **kwargs: object) -> object:
        raise AssertionError("Lookup attempted I/O or imports")

    with catalog_context(snapshot, expected_fingerprint=snapshot.fingerprint):
        monkeypatch.setattr(builtins, "open", forbidden)
        monkeypatch.setattr(Path, "open", forbidden)
        monkeypatch.setattr(sqlite3, "connect", forbidden)
        monkeypatch.setattr(importlib, "import_module", forbidden)
        assert dir(C.raw) == ["orders"]
        assert resolve_reference(
            C.old.items, expected_fingerprint=snapshot.fingerprint
        ).dataset_id == str(UUID(int=100))
        assert Output(C.raw.orders).ref == Input(C.raw.orders).ref


def test_installed_workers_keep_one_binding_across_threads_and_reject_test_override(
    installed: Path, tmp_path: Path
) -> None:
    def exercise(index: int, name: str) -> None:
        wire = document(index, name)
        other = document(index + 10, "unrelated")
        script = f"""
import json
from concurrent.futures import ThreadPoolExecutor
from transflow._catalog import _bind_worker
from transflow.catalog import C, CatalogSnapshot, CatalogContextError, resolve_reference
from transflow.testing import catalog_context
snapshot=CatalogSnapshot(json.loads({json.dumps(wire)!r}))
other=CatalogSnapshot(json.loads({json.dumps(other)!r}))
_bind_worker(snapshot, expected_fingerprint=snapshot.fingerprint)
def lookup(_):
    node=getattr(C.raw,{name!r})
    return resolve_reference(node,expected_fingerprint=snapshot.fingerprint).workspace_id
with ThreadPoolExecutor(max_workers=2) as pool:
    assert list(pool.map(lookup,range(4))) == [snapshot.workspace_id]*4
try:
    _bind_worker(other,expected_fingerprint=other.fingerprint)
except CatalogContextError:
    pass
else:
    raise AssertionError('Worker was rebound')
try:
    with catalog_context(other,expected_fingerprint=other.fingerprint):
        raise AssertionError('Worker context was overridden')
except CatalogContextError:
    pass
print(json.dumps([snapshot.workspace_id,dir(C.raw)]))
"""
        result = run([str(installed), "-I", "-B", "-c", script], tmp_path)
        assert json.loads(result.stdout) == [str(UUID(int=index)), [name]]

    with ThreadPoolExecutor(max_workers=2) as pool:
        futures = [pool.submit(exercise, 1, "orders"), pool.submit(exercise, 2, "invoices")]
        for future in futures:
            future.result(timeout=30)


@pytest.mark.parametrize("path", ["a" * 4097, "/".join(["a"] * 65)])
def test_namespace_index_limits_are_explicit(path: str) -> None:
    wire = document()
    wire["entries"][0]["path"] = path
    with pytest.raises(CatalogContextError):
        CatalogSnapshot(sign(wire))
