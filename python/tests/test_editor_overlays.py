"""Qualify the exact Rust-rendered fixtures with one installed SDK and isolated checkers."""

import ast
import hashlib
import json
import os
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any

from test_packaging import installed as installed
from test_packaging import run
from test_packaging import wheel as wheel

REPO = Path(__file__).resolve().parents[2]
CASES: list[dict[str, Any]] = json.loads(
    (REPO / "schemas/fixtures/editor-overlays-v1.json").read_text()
)


def activate(workspace: Path, case: dict[str, Any]) -> Path:
    """Install fixed Rust-qualified bytes; Rust tests independently exercise the writer."""
    generated = workspace / ".transflow/runtime/generated"
    destination = generated / case["snapshot"]["catalog_fingerprint"] / "type-stubs"
    for name, text in case["files"].items():
        path = destination / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    temporary = generated / ".next"
    temporary.symlink_to(destination.relative_to(generated))
    os.replace(temporary, generated / "current")
    return generated / "current"


def test_production_overlay_preserves_all_public_exports() -> None:
    import transflow
    import transflow.catalog

    for module, name in [(transflow, "__init__"), (transflow.catalog, "catalog")]:
        tree = ast.parse(CASES[0]["files"][f"transflow/{name}.pyi"])
        exported = {
            alias.asname
            for node in tree.body
            if isinstance(node, ast.ImportFrom)
            for alias in node.names
            if alias.asname
        }
        exported.update(
            node.target.id
            for node in tree.body
            if isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name)
        )
        assert set(module.__all__) <= exported


def test_installed_production_overlays_refresh_independently(
    installed: Path, tmp_path: Path
) -> None:
    before = {
        str(p): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in installed.parent.parent.rglob("*")
        if p.is_file() and not p.is_symlink()
    }

    def exercise(case: dict[str, Any], absent: str, refresh: bool) -> None:
        workspace = tmp_path / case["name"]
        workspace.mkdir()
        current = activate(workspace, case)
        config = workspace / "mypy.ini"
        config.write_text(
            f"[mypy]\nstrict = True\nmypy_path = {current}\n"
            f"python_executable = {installed}\ncache_dir = .mypy_cache\n"
        )
        consumer = workspace / "consumer.py"

        def check(name: str, missing: str) -> None:
            consumer.write_text(
                "from transflow import Input, Output, transform, Parameter, ProtocolVersion\n"
                "from transflow.catalog import (C, CatalogSnapshot, DatasetRef, "
                "CatalogContextError, CatalogLookupError, resolve_reference)\n"
                "from transflow.testing import catalog_context\n"
                "from transflow_worker import __version__\n"
                f"Input(C.raw.{name}); Output(C.legacy.{name})\n"
                f"reveal_type(C.raw.{name}.daily)\n"
                "s: CatalogSnapshot\nr: DatasetRef = resolve_reference(C.raw."
                f"{name}, expected_fingerprint='a' * 64)\n"
            )
            command = [sys.executable, "-m", "mypy", "--config-file", str(config), str(consumer)]
            assert "Revealed type" in run(command, workspace).stdout
            consumer.write_text(
                consumer.read_text() + f"C.raw.{missing}\nProtocolVersion('bad', 0)\n"
            )
            result = run(command, workspace, ok=False)
            assert result.returncode == 1
            assert "attr-defined" in result.stdout and "arg-type" in result.stdout
            assert "import-not-found" not in result.stdout and "import-untyped" not in result.stdout

        check(case["name"], absent)
        if refresh:
            assert activate(workspace, CASES[2]) == current
            check("shipments", "orders")
        run(
            [
                str(installed),
                "-I",
                "-B",
                "-c",
                f"import sys; sys.path.insert(0, {str(current)!r}); "
                "import transflow, transflow.catalog; from pathlib import Path; "
                f"assert not Path(transflow.__file__).is_relative_to({str(workspace)!r}); "
                f"assert not Path(transflow.catalog.__file__).is_relative_to({str(workspace)!r})",
            ],
            workspace,
        )

    with ThreadPoolExecutor(max_workers=2) as executor:
        results = [
            executor.submit(exercise, CASES[0], "invoices", True),
            executor.submit(exercise, CASES[1], "orders", False),
        ]
        for result in results:
            result.result(timeout=90)
    after = {
        str(p): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in installed.parent.parent.rglob("*")
        if p.is_file() and not p.is_symlink()
    }
    assert after == before
