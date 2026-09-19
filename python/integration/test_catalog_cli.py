"""Native CLI and actual installed matched worker, with no network or source imports."""

import hashlib
import json
import os
import sqlite3
import sys
from contextlib import closing
from pathlib import Path
from typing import Any
from uuid import uuid4

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


def test_browse_fresh_workspace_needs_no_python_environment(tmp_path: Path) -> None:
    root = tmp_path / "empty"
    cli(root, "init", str(root))
    before = files(root)
    value = cli(root, "catalog", "list")["result"]
    assert value["entries"] == [] and value["total"] == "0"
    assert files(root) == before
    assert cli(root, "catalog", "show", "unknown/path", ok=False)["exit_status"] == 1


def test_revision_bound_browse_and_retained_producer(workspace: Path) -> None:
    source(workspace, "orders.py", DECLARATION)
    source(workspace, "second.py", DECLARATION.replace("raw/orders", "raw/second"))
    cli(workspace, "catalog", "sync", "--python", sys.executable)
    first = cli(workspace, "catalog", "list", "--limit", "1")["result"]
    assert first["total"] == "2" and len(first["entries"]) == 1
    assert first["entries"][0]["version_count"] == "0"
    second = cli(workspace, "catalog", "list", "--limit", "1", "--cursor", first["next_cursor"])[
        "result"
    ]
    assert second["next_cursor"] is None
    assert first["entries"][0]["dataset_id"] != second["entries"][0]["dataset_id"]
    show = cli(workspace, "catalog", "show", "raw/orders")["result"]["entries"][0]
    assert show["producer"]["path"] == "src/orders.py"
    assert show["producer"]["stale"] is True
    source(workspace, "third.py", DECLARATION.replace("raw/orders", "raw/third"))
    cli(workspace, "catalog", "sync", "--python", sys.executable)
    assert (
        cli(workspace, "catalog", "list", "--cursor", first["next_cursor"], ok=False)["exit_status"]
        == 1
    )


@pytest.mark.parametrize("keep_alias", [False, True])
def test_rename_preserves_identity_and_reports_old_references(
    workspace: Path, keep_alias: bool
) -> None:
    source(workspace, "orders.py", DECLARATION)
    synced = cli(workspace, "catalog", "sync", "--python", sys.executable)["result"]
    identity = synced["registrations"]["entries"][0]["id"]

    # SQLite read-only WAL access may create coordination sidecars, never domain rows.
    def stable_files() -> dict[str, str]:
        return {
            p: digest
            for p, digest in files(workspace).items()
            if p
            not in {
                ".transflow/runtime/catalog.sqlite-wal",
                ".transflow/runtime/catalog.sqlite-shm",
            }
        }

    workspace_id = cli(workspace, "catalog", "show", "raw/orders")["result"]["entries"][0][
        "workspace_id"
    ]
    version_id, source_id = str(uuid4()), str(uuid4())
    with closing(sqlite3.connect(workspace / ".transflow/runtime/catalog.sqlite")) as db, db:
        db.execute("PRAGMA foreign_keys=ON")
        db.execute(
            "INSERT INTO source_snapshots VALUES(?,?,?,'{}',NULL,'{}','{}')",
            (source_id, workspace_id, "a" * 64),
        )
        db.execute("INSERT INTO artifacts VALUES(?,'{}','[]',0,0,0,'VERIFIED')", ("b" * 64,))
        db.execute(
            "INSERT INTO dataset_versions VALUES(?,?,?,NULL,?,?,1,?,?)",
            (version_id, identity, "b" * 64, str(uuid4()), source_id, "c" * 64, "d" * 64),
        )
    before = stable_files()
    args = ["catalog", "rename", "raw/orders", "curated/orders", "--python", sys.executable]
    if keep_alias:
        args.append("--keep-alias")
    preview = cli(workspace, *args)["result"]
    assert not preview["applied"] and preview["impact"]["total"] == "1"
    assert "C.raw.orders" in preview["impact"]["items"][0]["reference"]
    assert stable_files() == before
    changed = cli(workspace, *args, "--yes")["result"]
    assert changed["applied"] and changed["entries"][0]["dataset_id"] == identity
    assert (workspace / "src/orders.py").read_text() == DECLARATION
    historical = cli(workspace, "catalog", "show", f"dataset:{identity}")["result"]["entries"][0]
    assert historical["path"] == "curated/orders"
    assert historical["version_count"] == "1"
    assert historical["recent_versions"][0]["id"] == version_id
    assert historical["producer"]["path"] == "src/orders.py"
    old = cli(workspace, "catalog", "show", "raw/orders", ok=keep_alias)
    assert old["exit_status"] == (0 if keep_alias else 1)
    if keep_alias:
        assert (
            cli(workspace, "validate", "--python", sys.executable)["result"]["registrations"][
                "total"
            ]
            == "0"
        )


def test_remove_blocks_live_producer_then_retains_tombstone(workspace: Path) -> None:
    source(workspace, "orders.py", DECLARATION)
    identity = cli(workspace, "catalog", "sync", "--python", sys.executable)["result"][
        "registrations"
    ]["entries"][0]["id"]
    before = (workspace / ".transflow/catalog.toml").read_bytes()
    blocked = cli(
        workspace, "catalog", "remove", "raw/orders", "--yes", "--python", sys.executable, ok=False
    )
    assert blocked["result"]["blockers"]
    assert not blocked["result"]["applied"]
    assert (workspace / ".transflow/catalog.toml").read_bytes() == before
    (workspace / "src/orders.py").unlink()
    preview = cli(workspace, "catalog", "remove", "raw/orders", "--python", sys.executable)[
        "result"
    ]
    assert not preview["applied"]
    assert (workspace / ".transflow/catalog.toml").read_bytes() == before
    removed = cli(
        workspace, "catalog", "remove", "raw/orders", "--yes", "--python", sys.executable
    )["result"]
    assert removed["applied"] and removed["entries"][0]["tombstone"]
    show = cli(workspace, "catalog", "show", f"dataset:{identity}")["result"]["entries"][0]
    assert show["dataset_id"] == identity and show["tombstone"]
    source(workspace, "orders.py", DECLARATION)
    assert (
        cli(workspace, "catalog", "sync", "--python", sys.executable, ok=False)["exit_status"] == 1
    )


@pytest.mark.parametrize("runtime_reference", ["view", "schedule", "lease"])
def test_lifecycle_active_runtime_references_are_blocking(
    workspace: Path, runtime_reference: str
) -> None:
    source(workspace, "orders.py", DECLARATION)
    cli(workspace, "catalog", "sync", "--python", sys.executable)
    with closing(sqlite3.connect(workspace / ".transflow/runtime/catalog.sqlite")) as db, db:
        if runtime_reference == "view":
            db.execute("INSERT INTO graph_views VALUES(?,1,1,'{}','{}',1)", (str(uuid4()),))
        elif runtime_reference == "schedule":
            db.execute(
                "INSERT INTO schedules VALUES(?,'synthetic schedule',NULL,0,NULL)", (str(uuid4()),)
            )
        else:
            db.execute("INSERT INTO artifacts VALUES(?,'{}','[]',0,0,0,'VERIFIED')", ("e" * 64,))
            db.execute(
                "INSERT INTO read_leases "
                "VALUES(?,NULL,?,'synthetic lease',1,9223372036854775807,1)",
                (str(uuid4()), "e" * 64),
            )
    before = (workspace / ".transflow/catalog.toml").read_bytes()
    result = cli(
        workspace,
        "catalog",
        "rename",
        "raw/orders",
        "new/orders",
        "--yes",
        "--python",
        sys.executable,
        ok=False,
    )
    assert result["exit_status"] == 1 and result["result"]["blockers"]
    assert (workspace / ".transflow/catalog.toml").read_bytes() == before
