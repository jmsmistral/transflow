"""Optional actual native Emacs client; source/protocol suites stay portable in CI."""

import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest
from test_catalog_cli import DECLARATION, cli, source
from test_catalog_cli import workspace as workspace
from test_environment_service import active
from test_lsp import simple
from test_packaging import REPOSITORY
from test_packaging import wheel as wheel


def test_native_emacs_client(tmp_path: Path, workspace: Path) -> None:
    emacs = os.environ.get("TRANSFLOW_EMACS") or shutil.which("emacs")
    if not emacs:
        pytest.skip("Native Emacs unavailable; no client qualification is inferred")
    root_a = simple(tmp_path / "a", "orders")
    root_b = simple(tmp_path / "b", "invoices")
    source(workspace, "orders.py", DECLARATION)
    cli(workspace, "catalog", "sync", "--python", sys.executable)
    env = {
        **os.environ,
        "TRANSFLOW_CLI": str(REPOSITORY / "target/debug/transflow"),
        "TRANSFLOW_PYTHON": sys.executable,
        "TRANSFLOW_EDITOR_A": str(root_a),
        "TRANSFLOW_EDITOR_B": str(root_b),
        "TRANSFLOW_EDITOR_PRODUCER": str(workspace),
        "TRANSFLOW_INSTALLED_PYTHON": str(active(workspace) / "bin/python"),
    }
    result = subprocess.run(
        [
            emacs,
            "--batch",
            "-Q",
            "-L",
            str(REPOSITORY / "editors/emacs"),
            "-l",
            "transflow-test.el",
            "-f",
            "ert-run-tests-batch-and-exit",
        ],
        env=env,
        text=True,
        capture_output=True,
        check=False,
        timeout=45,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert "4 results as expected" in result.stderr


def test_vscode_transport_with_native_service(tmp_path: Path) -> None:
    node = shutil.which("node")
    if not node:
        pytest.skip("Node unavailable; native VS Code qualification is a separate check")
    root = simple(tmp_path / "node", "orders")
    result = subprocess.run(
        [node, "--test", str(REPOSITORY / "editors/vscode/transport.test.cjs")],
        env={
            **os.environ,
            "TRANSFLOW_CLI": str(REPOSITORY / "target/debug/transflow"),
            "TRANSFLOW_PYTHON": sys.executable,
            "TRANSFLOW_EDITOR_A": str(root),
        },
        text=True,
        capture_output=True,
        check=False,
        timeout=30,
    )
    assert result.returncode == 0, result.stdout + result.stderr
