"""Native CLI and actual installed matched worker, with no network or source imports."""

import hashlib
import json
import os
import sys
from pathlib import Path
from typing import Any

import pytest
from test_environment_service import MINOR, active, fixture_wheel, lock, sync
from test_packaging import REPOSITORY, run
from test_packaging import wheel as wheel
from transflow_worker.wire import validate_document

CLI = Path(os.environ.get("TRANSFLOW_CLI", REPOSITORY / "target/debug/transflow"))


def cli(root: Path, *args: str, ok: bool = True) -> dict[str, Any]:
    result = run([str(CLI), "--workspace", str(root), "--json", *args], root.parent, ok=ok)
    value: dict[str, Any] = json.loads(result.stdout)
    validate_document("CliEnvelopeV1", value)
    assert value["exit_status"] == result.returncode
    assert not result.stderr
    return value


@pytest.fixture
def workspace(tmp_path: Path, wheel: Path) -> Path:
    root = tmp_path / "workspace"
    cli(root, "init", str(root))
    config = root / "workspace.toml"
    config.write_text(config.read_text().replace('version = "3.14"', f'version = "{MINOR}"'))
    (root / "wheels").mkdir()
    fixture_wheel(root / "wheels", "transflow_fixture_leaf", "1.0.0")
    (root / "requirements.in").write_text("transflow-fixture-leaf==1.0.0\n")
    lock(root)
    sync(root, wheel)
    return root


def files(root: Path) -> dict[str, str]:
    return {
        str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in root.rglob("*")
        if p.is_file()
    }


def source(root: Path, name: str, code: str) -> None:
    (root / "src" / name).write_text(code)


DECLARATION = """from transflow import transform, Output
@transform(output=Output("raw/orders"))
def orders():
    raise AssertionError("Producer must never run during preparation")
"""


def test_validate_check_sync_and_idempotence(workspace: Path) -> None:
    source(workspace, "orders.py", DECLARATION)
    before = files(workspace)
    result = cli(workspace, "validate", "--python", sys.executable)["result"]
    assert result["registrations"]["total"] == "1"
    assert result["registrations"]["entries"][0]["id"] is None
    assert files(workspace) == before
    check = cli(workspace, "catalog", "sync", "--check", "--python", sys.executable, ok=False)
    assert check["exit_status"] == 1
    assert check["result"]["registrations"] == result["registrations"]
    assert files(workspace) == before
    synced = cli(workspace, "catalog", "sync", "--python", sys.executable)["result"]
    assert synced["registrations"]["entries"][0]["id"]
    registry = (workspace / ".transflow/catalog.toml").read_bytes()
    assert (workspace / ".transflow/runtime/generated/current/transflow/catalog.pyi").is_file()
    assert (workspace / ".transflow/runtime/graphs/current").is_file()
    again = cli(workspace, "catalog", "sync", "--python", sys.executable)["result"]
    assert again["registrations"]["total"] == "0"
    assert again["unchanged"]["total"] == "1"
    assert (workspace / ".transflow/catalog.toml").read_bytes() == registry
    before = files(workspace)
    assert (
        cli(workspace, "catalog", "sync", "--check", "--python", sys.executable)["exit_status"] == 0
    )
    assert files(workspace) == before


def test_invalid_graph_keeps_registry_and_last_valid_graph(workspace: Path) -> None:
    source(workspace, "orders.py", DECLARATION)
    cli(workspace, "catalog", "sync", "--python", sys.executable)
    registry = (workspace / ".transflow/catalog.toml").read_bytes()
    graph = (workspace / ".transflow/runtime/graphs/current").read_bytes()
    source(
        workspace,
        "broken.py",
        """from transflow import transform, Input, Output
@transform(missing=Input("does/not/exist"), output=Output("new/output"))
def broken(missing): return missing
""",
    )
    failed = cli(workspace, "catalog", "sync", "--python", sys.executable, ok=False)
    assert failed["exit_status"] == 1
    assert (workspace / ".transflow/catalog.toml").read_bytes() == registry
    assert (workspace / ".transflow/runtime/graphs/current").read_bytes() == graph


def test_absent_producer_retained_and_human_json_agree(workspace: Path) -> None:
    source(workspace, "orders.py", DECLARATION)
    cli(workspace, "catalog", "sync", "--python", sys.executable)
    (workspace / "src/orders.py").unlink()
    before = (workspace / ".transflow/catalog.toml").read_bytes()
    result = cli(workspace, "catalog", "sync", "--python", sys.executable)["result"]
    assert result["absent_producers"]["entries"][0]["path"] == "raw/orders"
    assert (workspace / ".transflow/catalog.toml").read_bytes() == before
    human = run(
        [str(CLI), "--workspace", str(workspace), "validate", "--python", sys.executable],
        workspace.parent,
    )
    assert "Absent producers (retained): 1" in human.stdout
    assert "raw/orders" in human.stdout


@pytest.mark.parametrize(
    "code", ["raise RuntimeError('private exception text')\n", "this is not valid python !\n"]
)
def test_import_and_syntax_fail_read_only(workspace: Path, code: str) -> None:
    source(workspace, "broken.py", code)
    before = files(workspace)
    result = cli(workspace, "validate", "--python", sys.executable, ok=False)
    assert result["exit_status"] == 1
    assert "private exception text" not in json.dumps(result)
    assert "broken.py" in json.dumps(result)
    assert files(workspace) == before


def test_environment_drift_fails_before_import(workspace: Path) -> None:
    source(workspace, "orders.py", "raise RuntimeError('must not import')\n")
    package = (
        active(workspace)
        / "lib"
        / f"python{MINOR}"
        / "site-packages/transflow_fixture_leaf/__init__.py"
    )
    package.write_text("VALUE='changed'\n")
    before = files(workspace)
    result = cli(workspace, "validate", "--python", sys.executable, ok=False)
    assert result["exit_status"] == 1
    assert "drift" in json.dumps(result)
    assert files(workspace) == before


def test_forward_references_bound_c_and_whole_graph_cycle(workspace: Path) -> None:
    source(workspace, "zzz.py", DECLARATION)
    source(
        workspace,
        "aaa.py",
        """from transflow import transform, Input, Output
@transform(orders=Input("raw/orders"), output=Output("curated/orders"))
def curated(orders): return orders
""",
    )
    cli(workspace, "catalog", "sync", "--python", sys.executable)
    source(
        workspace,
        "aaa.py",
        """from transflow import transform, Input, Output
from transflow.catalog import C
@transform(orders=Input(C.raw.orders), output=Output(C.curated.orders))
def curated(orders): return orders
""",
    )
    # Rebinding an existing C identity during an additive sync does not rerun imports.
    source(workspace, "new.py", DECLARATION.replace("raw/orders", "new/orders"))
    result = cli(workspace, "catalog", "sync", "--python", sys.executable)["result"]
    assert result["registrations"]["total"] == "1"
    source(
        workspace,
        "cycle_a.py",
        """from transflow import transform, Input, Output
@transform(other=Input("cycle/b"), output=Output("cycle/a"))
def a(other): return other
""",
    )
    source(
        workspace,
        "cycle_b.py",
        """from transflow import transform, Input, Output
@transform(other=Input("cycle/a"), output=Output("cycle/b"))
def b(other): return other
""",
    )
    before = (workspace / ".transflow/catalog.toml").read_bytes()
    failed = cli(workspace, "catalog", "sync", "--python", sys.executable, ok=False)
    assert failed["diagnostics"][0]["code"] == "TF_GRAPH_CYCLE"
    assert (workspace / ".transflow/catalog.toml").read_bytes() == before


def test_source_edit_during_import_stops_registration(workspace: Path) -> None:
    source(workspace, "orders.py", DECLARATION)
    source(
        workspace,
        "edit.py",
        f"""from pathlib import Path
Path({str(workspace / "src/orders.py")!r}).write_text("# changed during capture import\\n")
""",
    )
    before = (workspace / ".transflow/catalog.toml").read_bytes()
    failed = cli(workspace, "catalog", "sync", "--python", sys.executable, ok=False)
    assert failed["exit_status"] == 1
    assert (workspace / ".transflow/catalog.toml").read_bytes() == before
    assert not (workspace / ".transflow/runtime/graphs/current").exists()


def test_helper_edit_invalidates_certificate_without_registering_again(workspace: Path) -> None:
    source(workspace, "orders.py", DECLARATION)
    source(workspace, "helper.py", "VALUE = 1\n")
    old = cli(workspace, "catalog", "sync", "--python", sys.executable)["result"]
    source(workspace, "helper.py", "VALUE = 2\n")
    new = cli(workspace, "catalog", "sync", "--python", sys.executable)["result"]
    assert new["registrations"]["total"] == "0"
    assert old["certificate_fingerprint"] != new["certificate_fingerprint"]
