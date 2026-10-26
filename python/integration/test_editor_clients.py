"""Opt-in native editor clients; source/protocol suites stay portable in CI."""

import hashlib
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
from collections.abc import Iterator
from contextlib import suppress
from pathlib import Path

import pytest
from package_vscode import package
from test_catalog_cli import DECLARATION, cli, source
from test_catalog_cli import workspace as workspace
from test_environment_service import active, fixture_wheel, lock, sync
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


@pytest.fixture
def native_editor_profile() -> Iterator[Path]:
    """Keep the desktop IPC socket below macOS's Unix-socket path limit."""
    with tempfile.TemporaryDirectory(prefix="tf-code-", dir="/tmp") as directory:
        yield Path(directory).resolve()


def test_native_vscode_extension_host(
    tmp_path: Path, workspace: Path, wheel: Path, native_editor_profile: Path
) -> None:
    """Opt-in desktop host test; normal CI must not download or emulate an editor."""
    executable = os.environ.get("TRANSFLOW_VSCODE")
    editor_cli = os.environ.get("TRANSFLOW_VSCODE_CLI")
    if not executable or not editor_cli:
        pytest.skip("Set TRANSFLOW_VSCODE and TRANSFLOW_VSCODE_CLI for native host qualification")
    root_b = tmp_path / "invoices"
    cli(root_b, "init", str(root_b))
    (root_b / "wheels").mkdir()
    # Editor/package fixture only; this test must never execute an engine.
    fixture_wheel(root_b / "wheels", "duckdb", "1.5.5")
    fixture_wheel(root_b / "wheels", "transflow_fixture_leaf", "1.0.0")
    (root_b / "requirements.in").write_text("transflow-fixture-leaf==1.0.0\n")
    lock(root_b)
    sync(root_b, wheel)
    source(root_b, "invoices.py", DECLARATION.replace("orders", "invoices"))
    cli(root_b, "catalog", "sync", "--python", sys.executable)
    source(workspace, "orders.py", DECLARATION)
    cli(workspace, "catalog", "sync", "--python", sys.executable)
    source(workspace, "editor-draft.py", "from transflow.catalog import C\nC.raw.orders\n")
    source(root_b, "editor-draft.py", "from transflow.catalog import C\nC.raw.invoices\n")

    profile = native_editor_profile
    extensions = tmp_path / "extensions"
    profile_settings = profile / "User/settings.json"
    profile_settings.parent.mkdir(parents=True)
    profile_settings.write_text(
        json.dumps(
            {
                "telemetry.telemetryLevel": "off",
                "extensions.autoUpdate": False,
                "extensions.autoCheckUpdates": False,
                "update.mode": "none",
                "workbench.startupEditor": "none",
                "workbench.enableExperiments": False,
                "editor.wordBasedSuggestions": "off",
                "python.languageServer": "None",
                "basedpyright.importStrategy": "useBundled",
                "ruff.importStrategy": "useBundled",
            }
        )
    )
    extra_extensions = os.environ.get("TRANSFLOW_VSCODE_TEST_EXTENSIONS")
    if extra_extensions:
        shutil.copytree(extra_extensions, extensions)
    else:
        extensions.mkdir()
    vsix = package(REPOSITORY / "editors/vscode", tmp_path / "transflow.vsix")
    installed = subprocess.run(
        [
            editor_cli,
            "--user-data-dir",
            str(profile),
            "--extensions-dir",
            str(extensions),
            "--install-extension",
            str(vsix),
            "--force",
        ],
        capture_output=True,
        text=True,
        check=False,
        timeout=60,
    )
    assert installed.returncode == 0, installed.stdout + installed.stderr
    helper = tmp_path / "test-host"
    helper.mkdir()
    (helper / "package.json").write_text(
        json.dumps(
            {
                "name": "transflow-native-test-host",
                "publisher": "transflow-local",
                "version": "0.0.0",
                "engines": {"vscode": "^1.100.0"},
                "main": "main.cjs",
            }
        )
    )
    (helper / "main.cjs").write_text("exports.activate = () => {};\n")
    native_workspace = tmp_path / "native.code-workspace"
    for root in (workspace, root_b):
        settings = root / ".vscode/settings.json"
        settings.parent.mkdir()
        settings.write_text(
            json.dumps(
                {
                    "python.defaultInterpreterPath": str(active(root) / "bin/python"),
                }
            )
        )
    native_workspace.write_text(
        json.dumps(
            {
                "folders": [{"path": str(workspace)}, {"path": str(root_b)}],
                "settings": {
                    "transflow.executable": str(REPOSITORY / "target/debug/transflow"),
                    "transflow.python": sys.executable,
                },
            }
        )
    )
    sdks = [active(root) / "lib/python3.14/site-packages/transflow" for root in (workspace, root_b)]

    def sdk_bytes() -> dict[str, str]:
        return {
            str(p): hashlib.sha256(p.read_bytes()).hexdigest()
            for sdk in sdks
            for p in sdk.rglob("*")
            if p.is_file() and p.suffix in (".py", ".pyi")
        }

    before = sdk_bytes()
    assert all((sdk / "__init__.py").is_file() for sdk in sdks)
    report_path = Path(os.environ.get("TRANSFLOW_EDITOR_REPORT", tmp_path / "native-report.json"))
    report_path.parent.mkdir(parents=True, exist_ok=True)
    environment = {
        **os.environ,
        "TRANSFLOW_EDITOR_A": str(workspace),
        "TRANSFLOW_EDITOR_B": str(root_b),
        "TRANSFLOW_INSTALLED_EXTENSIONS": str(extensions),
        "TRANSFLOW_EDITOR_REPORT": str(report_path),
    }
    command = [
        executable,
        "--user-data-dir",
        str(profile),
        "--extensions-dir",
        str(extensions),
        "--skip-welcome",
        "--skip-release-notes",
        "--disable-workspace-trust",
        "--skip-add-to-recently-opened",
        "--disable-crash-reporter",
        f"--extensionDevelopmentPath={helper}",
        f"--extensionTestsPath={REPOSITORY / 'editors/vscode/host.test.cjs'}",
        str(native_workspace),
    ]
    with subprocess.Popen(
        command,
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    ) as process:
        try:
            output, errors = process.communicate(timeout=180)
        except subprocess.TimeoutExpired:
            with suppress(ProcessLookupError):
                os.killpg(process.pid, signal.SIGTERM)
            try:
                process.communicate(timeout=15)
            except subprocess.TimeoutExpired:
                with suppress(ProcessLookupError):
                    os.killpg(process.pid, signal.SIGKILL)
                process.communicate(timeout=15)
            raise
    assert process.returncode == 0, output + errors
    report = json.loads(report_path.read_text())
    assert report["outcome"] == "passed", report
    assert len(report["checks"]) >= 7
    assert before == sdk_bytes()
    for root, sdk in zip((workspace, root_b), sdks, strict=True):
        imported = subprocess.run(
            [
                str(active(root) / "bin/python"),
                "-I",
                "-c",
                "import json, sys, transflow, transflow.catalog; "
                "print(json.dumps([transflow.__file__, transflow.catalog.__file__, sys.path]))",
            ],
            capture_output=True,
            text=True,
            check=True,
            timeout=15,
        )
        runtime, catalogue, paths = json.loads(imported.stdout)
        assert Path(runtime).is_relative_to(sdk)
        assert Path(catalogue).is_relative_to(sdk)
        assert not any("runtime/generated" in entry for entry in paths)
    assert not (workspace / "must-not-execute").exists()
