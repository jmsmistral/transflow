"""Synthetic T010 snapshot and typing-overlay fixtures; no product catalogue schema."""

import ast
import asyncio
from dataclasses import FrozenInstanceError
from pathlib import Path
from uuid import UUID

import pytest
import transflow
from catalog_overlay import render, verify, write_overlay
from transflow._catalog_prototype import (
    CatalogContextError,
    CatalogLookupError,
    PrototypeSnapshot,
    resolve_reference,
)
from transflow.catalog import C
from transflow.testing import catalog_context


def snapshot(*paths: str, workspace: int = 1) -> PrototypeSnapshot:
    return PrototypeSnapshot(
        str(UUID(int=workspace)),
        tuple((path, str(UUID(int=index + 100))) for index, path in enumerate(paths)),
    )


def test_snapshot_identity_order_and_immutability() -> None:
    original = snapshot("raw/orders", "raw/orders/daily")
    reordered = PrototypeSnapshot(original.workspace_id, tuple(reversed(original.entries)))
    assert original == reordered
    assert original.fingerprint == reordered.fingerprint
    assert (
        original.fingerprint != snapshot("raw/orders", "raw/orders/daily", workspace=2).fingerprint
    )
    with pytest.raises(FrozenInstanceError):
        original.workspace_id = str(UUID(int=2))  # type: ignore[misc]


@pytest.mark.parametrize(
    "path", ["", "/raw", "raw/", "raw//orders", "_raw", "Raw", "a-b", "class", "raw/for"]
)
def test_invalid_paths(path: str) -> None:
    with pytest.raises(ValueError, match="identifiers"):
        snapshot(path)


def test_duplicate_and_invalid_identities() -> None:
    good = snapshot("raw/orders")
    with pytest.raises(ValueError, match="Duplicate"):
        snapshot("raw/orders", "raw/orders")
    with pytest.raises(ValueError, match="Duplicate"):
        PrototypeSnapshot(good.workspace_id, (good.entries[0], ("raw/other", good.entries[0][1])))
    with pytest.raises(ValueError):
        PrototypeSnapshot("invalid", good.entries)
    with pytest.raises(ValueError):
        PrototypeSnapshot(good.workspace_id, (("raw/orders", "invalid"),))


def test_prefix_datasets_and_metadata_names() -> None:
    captured = snapshot(
        "raw/orders", "raw/orders/daily", "raw/path", "raw/fingerprint", "raw/dataset_id"
    )
    with catalog_context(captured, expected_fingerprint=captured.fingerprint):
        assert dir(C) == ["raw"]
        assert dir(C.raw) == ["dataset_id", "fingerprint", "orders", "path"]
        for path, identity in captured.entries:
            node = C.raw
            for segment in path.split("/")[1:]:
                node = getattr(node, segment)
            ref = resolve_reference(node, expected_fingerprint=captured.fingerprint)
            assert (ref.path, ref.dataset_id, ref.workspace_id) == (
                path,
                identity,
                captured.workspace_id,
            )
        with pytest.raises(CatalogContextError, match="namespace"):
            resolve_reference(C.raw, expected_fingerprint=captured.fingerprint)
        with pytest.raises(CatalogLookupError, match="Declare a new output using a string"):
            _ = C.raw.missing
        with pytest.raises(FrozenInstanceError):
            C.raw._path = "wrong"  # type: ignore[misc]


def test_context_restoration_and_stale_references() -> None:
    alpha, beta = snapshot("raw/orders"), snapshot("raw/invoices", workspace=2)
    assert dir(C) == []
    with pytest.raises(CatalogContextError, match="No catalogue"):
        _ = C.raw
    with catalog_context(alpha, expected_fingerprint=alpha.fingerprint):
        captured = C.raw.orders
        with pytest.raises(CatalogContextError, match="stale"):
            with catalog_context(beta, expected_fingerprint=alpha.fingerprint):
                pytest.fail("Mismatched snapshot was bound")
        assert dir(C.raw) == ["orders"]
        with pytest.raises(RuntimeError, match="consumer failed"):
            with catalog_context(beta, expected_fingerprint=beta.fingerprint):
                assert dir(C.raw) == ["invoices"]
                with pytest.raises(CatalogContextError, match="stale"):
                    resolve_reference(captured, expected_fingerprint=beta.fingerprint)
                raise RuntimeError("consumer failed")
        assert dir(C.raw) == ["orders"]
    assert resolve_reference(captured, expected_fingerprint=alpha.fingerprint).path == "raw/orders"
    assert dir(C) == []


def test_two_simultaneous_contexts() -> None:
    async def exercise() -> None:
        ready = [asyncio.Event(), asyncio.Event()]

        async def consumer(index: int, name: str) -> None:
            captured = snapshot(f"raw/{name}", workspace=index + 1)
            with catalog_context(captured, expected_fingerprint=captured.fingerprint):
                ready[index].set()
                await asyncio.wait_for(ready[1 - index].wait(), timeout=5)
                assert dir(C.raw) == [name]
                ref = resolve_reference(
                    getattr(C.raw, name), expected_fingerprint=captured.fingerprint
                )
                assert ref.workspace_id == captured.workspace_id

        await asyncio.gather(consumer(0, "orders"), consumer(1, "invoices"))
        assert dir(C) == []

    asyncio.run(exercise())


def test_overlay_generations_are_immutable_and_workspace_local(tmp_path: Path) -> None:
    alpha, beta = snapshot("raw/orders"), snapshot("raw/invoices", workspace=2)
    first = write_overlay(tmp_path, alpha)
    before = {name: (first / name).read_bytes() for name in render(alpha)}
    assert write_overlay(tmp_path, alpha) == first
    second = write_overlay(tmp_path, beta)
    assert first != second
    assert {name: (first / name).read_bytes() for name in render(alpha)} == before
    assert not list(tmp_path.rglob("*.py"))
    assert not list(tmp_path.rglob(".overlay-*"))
    with pytest.raises(CatalogContextError, match="stale"):
        verify(first, beta)
    (first / "transflow/catalog.pyi").write_text("tampered\n")
    with pytest.raises(CatalogContextError, match="content differs"):
        write_overlay(tmp_path, alpha)
    assert (first / "transflow/catalog.pyi").read_text() == "tampered\n"
    verify(second, beta)


def test_overlay_rejects_extra_files_and_symlinks(tmp_path: Path) -> None:
    captured = snapshot("raw/orders")
    overlay = write_overlay(tmp_path, captured)
    unexpected = overlay / "transflow/__init__.py"
    unexpected.write_text("raise RuntimeError('shadow')\n")
    with pytest.raises(CatalogContextError, match="unexpected files"):
        verify(overlay, captured)
    unexpected.unlink()
    target = overlay / "transflow/catalog.pyi"
    target.unlink()
    target.symlink_to(tmp_path / "absent")
    with pytest.raises(ValueError, match="symlink"):
        verify(overlay, captured)


def test_overlay_rejects_symlink_escape(tmp_path: Path) -> None:
    outside = tmp_path / "outside"
    workspace = tmp_path / "workspace"
    outside.mkdir()
    workspace.mkdir()
    (workspace / ".transflow").symlink_to(outside, target_is_directory=True)
    with pytest.raises(ValueError, match="symlinks"):
        write_overlay(workspace, snapshot("raw/orders"))
    assert list(outside.iterdir()) == []


def test_failed_generation_preserves_previous_bytes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alpha, beta = snapshot("raw/orders"), snapshot("raw/invoices", workspace=2)
    first = write_overlay(tmp_path, alpha)

    def fail_rename(self: Path, target: Path) -> Path:
        raise OSError("injected rename failure")

    monkeypatch.setattr(Path, "rename", fail_rename)
    with pytest.raises(OSError, match="injected"):
        write_overlay(tmp_path, beta)
    verify(first, alpha)
    assert not list(tmp_path.rglob(".overlay-*"))
    assert not (
        tmp_path / ".transflow/runtime/generated" / beta.fingerprint / "type-stubs"
    ).exists()


def test_empty_overlay_and_root_export_drift(tmp_path: Path) -> None:
    captured = snapshot()
    overlay = write_overlay(tmp_path, captured)
    verify(overlay, captured)
    root_stub = ast.parse((overlay / "transflow/__init__.pyi").read_text())
    exports = {
        alias.asname
        for node in root_stub.body
        if isinstance(node, ast.ImportFrom)
        for alias in node.names
    }
    assert exports == set(transflow.__all__)
    with catalog_context(captured, expected_fingerprint=captured.fingerprint):
        assert dir(C) == []
        with pytest.raises(CatalogLookupError):
            _ = C.raw
