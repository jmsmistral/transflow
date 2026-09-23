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
                "(id,version_id,artifact_digest,owner_operation,renewed_at_us,expires_at_us,fence) "
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


def parquet_input(workspace: Path, name: str = "input.parquet", empty: bool = False) -> Path:
    root = workspace.parent / "data"
    root.mkdir(exist_ok=True)
    path = root / name
    fixture = "empty.parquet" if empty else "two-rows.parquet"
    path.write_bytes((REPOSITORY / "tests/fixtures/import" / fixture).read_bytes())
    return path


def prepare_import(
    workspace: Path, path: Path, target: str = "raw/imported", *, ok: bool = True
) -> dict[str, Any]:
    return cli(
        workspace,
        "dataset",
        "import",
        target,
        "--path",
        str(path),
        "--prepare-only",
        "--python",
        sys.executable,
        ok=ok,
    )


@pytest.mark.parametrize("bound", [False, True])
def test_import_registers_explicit_input_without_publication(workspace: Path, bound: bool) -> None:
    path = parquet_input(workspace)
    reference = "C.raw.imported" if bound else '"raw/imported"'
    source(
        workspace,
        "consumer.py",
        f"""from transflow import transform, Input, Output
from transflow.catalog import C
@transform(data=Input({reference}), output=Output("curated/output"))
def consumer(data): raise AssertionError("Import preparation must not call producers")
""",
    )
    result = prepare_import(workspace, path)["result"]
    assert result["status"] == "prepared" and result["published"] is False
    assert result["registered"] and result["row_count"] == "2"
    assert result["schema_normalization"] == "complete"
    staging = workspace / result["staging_path"]
    manifest = json.loads((staging / "prepared.json").read_text())
    validate_document("ImportStagingManifestV1", manifest)
    assert manifest["source_files"] == ["input.parquet"]
    assert manifest["logical_schema"]["fields"][0]["logical_type"] == {"type": "i64"}
    assert len(manifest["schema_fingerprint"]) == 64
    assert (staging / "part-00000.parquet").read_bytes() == path.read_bytes()
    assert (staging / "part-00000.parquet").stat().st_ino != path.stat().st_ino
    with closing(sqlite3.connect(workspace / ".transflow/runtime/catalog.sqlite")) as db:
        assert db.execute("SELECT count(*) FROM dataset_versions").fetchone()[0] == 0
        assert db.execute("SELECT count(*) FROM data_branches").fetchone()[0] == 0
    assert (
        cli(workspace, "catalog", "show", "raw/imported")["result"]["entries"][0]["kind"]
        == "imported"
    )
    again = prepare_import(workspace, path, f"dataset:{result['dataset_id']}")["result"]
    assert not again["registered"] and again["dataset_id"] == result["dataset_id"]
    path.write_bytes(b"source replaced after preparation")
    assert (staging / "part-00000.parquet").read_bytes() != path.read_bytes()


def test_zero_row_parquet_and_frozen_sorted_multifile_import(workspace: Path) -> None:
    empty = parquet_input(workspace, "a.parquet", empty=True)
    assert prepare_import(workspace, empty)["result"]["row_count"] == "0"
    parquet_input(workspace, "b.parquet")
    result = prepare_import(workspace, empty.parent / "*.parquet")["result"]
    assert result["file_count"] == "2" and result["row_count"] == "2"
    manifest = json.loads((workspace / result["staging_path"] / "prepared.json").read_text())
    assert manifest["source_files"] == ["a.parquet", "b.parquet"]
    assert [f["row_count"] for f in manifest["files"]] == ["0", "2"]


@pytest.mark.parametrize("invalid", ["empty_glob", "empty_file", "symlink", "unsupported_type"])
def test_invalid_import_sources_do_not_register(workspace: Path, invalid: str) -> None:
    original = parquet_input(workspace)
    if invalid == "empty_glob":
        path = original.parent / "*.absent"
    elif invalid == "unsupported_type":
        path = original
        path.write_bytes(
            (REPOSITORY / "tests/fixtures/import/unsupported-duration.parquet").read_bytes()
        )
    elif invalid == "empty_file":
        path = original
        path.write_bytes(b"")
    else:
        path = original.parent / "link.parquet"
        path.symlink_to(original)
    before = (workspace / ".transflow/catalog.toml").read_bytes()
    assert prepare_import(workspace, path, ok=False)["exit_status"] == 1
    assert (workspace / ".transflow/catalog.toml").read_bytes() == before


@pytest.mark.parametrize("registered", [False, True])
def test_import_cannot_take_over_a_python_producer(workspace: Path, registered: bool) -> None:
    source(workspace, "orders.py", DECLARATION)
    if registered:
        cli(workspace, "catalog", "sync", "--python", sys.executable)
    before = (workspace / ".transflow/catalog.toml").read_bytes()
    assert (
        prepare_import(workspace, parquet_input(workspace), "raw/orders", ok=False)["exit_status"]
        == 1
    )
    assert (workspace / ".transflow/catalog.toml").read_bytes() == before


def test_source_mutation_during_preparation_is_caught(workspace: Path) -> None:
    path = parquet_input(workspace)
    source(
        workspace,
        "change.py",
        f"""from pathlib import Path
Path({str(path)!r}).write_bytes(b"changed while preparing")
""",
    )
    before = (workspace / ".transflow/catalog.toml").read_bytes()
    failed = prepare_import(workspace, path, ok=False)
    assert failed["exit_status"] == 1
    assert (workspace / ".transflow/catalog.toml").read_bytes() == before
    assert not list((workspace / ".transflow/runtime/import-staging").iterdir())


def test_import_requires_explicit_preparation_mode(workspace: Path) -> None:
    before = files(workspace)
    result = cli(
        workspace, "dataset", "import", "raw/input", "--path", "anything.parquet", ok=False
    )
    assert result["exit_status"] == 1
    assert "--prepare-only" in json.dumps(result)
    assert files(workspace) == before


def plan_probe(root: Path, operation: str, reference: str, *args: str) -> dict[str, Any]:
    probe = CLI.parent / "examples/planning_probe"
    result = run([str(probe), str(root), sys.executable, operation, reference, *args], root)
    value: dict[str, Any] = json.loads(result.stdout)
    return value


def test_saved_plan_proposed_ids_symbolic_parents_and_exact_acceptance(workspace: Path) -> None:
    source(workspace, "orders.py", DECLARATION)
    source(
        workspace,
        "consumer.py",
        "from transflow import transform, Input, Output\n"
        '@transform(rows=Input("raw/orders"), output=Output("curated/orders"))\n'
        'def consumer(rows): raise AssertionError("must not execute")\n',
    )
    registry = workspace / ".transflow/catalog.toml"
    before = registry.read_bytes()
    prepared = plan_probe(workspace, "full", "curated/orders")
    assert prepared["ok"], prepared
    draft = prepared["result"]["plan"]
    assert registry.read_bytes() == before
    assert len(draft["writes"]) == 2
    assert draft["reads"] == []
    assert draft["writes"][1]["bindings"][0]["kind"] == "planned"
    with closing(sqlite3.connect(workspace / ".transflow/runtime/catalog.sqlite")) as db:
        assert db.execute("SELECT count(*) FROM datasets").fetchone()[0] == 0
        assert db.execute("SELECT count(*) FROM data_branches").fetchone()[0] == 0
    accepted = plan_probe(workspace, "accept", draft["id"])
    assert accepted["ok"], accepted
    assert accepted["result"]["plan"] == draft
    assert registry.read_text() == draft["replacement"]
    with closing(sqlite3.connect(workspace / ".transflow/runtime/catalog.sqlite")) as db:
        assert db.execute("SELECT count(*) FROM jobs").fetchone()[0] == 2
        assert db.execute("SELECT count(*) FROM write_reservations").fetchone()[0] == 2
        assert db.execute("SELECT count(*) FROM dataset_versions").fetchone()[0] == 0
    assert not plan_probe(workspace, "accept", draft["id"])["ok"]


@pytest.mark.parametrize("changed", ["source", "registry", "environment"])
def test_saved_plan_conflicts_preserve_registry_and_start_no_jobs(
    workspace: Path, changed: str
) -> None:
    source(workspace, "orders.py", DECLARATION)
    prepared = plan_probe(workspace, "full", "raw/orders")
    assert prepared["ok"], prepared
    draft = prepared["result"]["plan"]
    if changed == "source":
        source(workspace, "orders.py", DECLARATION + "# changed after planning\n")
    elif changed == "registry":
        registry = workspace / ".transflow/catalog.toml"
        registry.write_text(registry.read_text() + "# owner edit\n")
    else:
        package = (
            active(workspace)
            / "lib"
            / f"python{MINOR}"
            / "site-packages/transflow_fixture_leaf/__init__.py"
        )
        package.write_text("VALUE='changed'\n")
    before = (workspace / ".transflow/catalog.toml").read_bytes()
    assert not plan_probe(workspace, "accept", draft["id"])["ok"]
    assert (workspace / ".transflow/catalog.toml").read_bytes() == before
    with closing(sqlite3.connect(workspace / ".transflow/runtime/catalog.sqlite")) as db:
        assert db.execute("SELECT count(*) FROM jobs").fetchone()[0] == 0
        assert db.execute("SELECT count(*) FROM data_branches").fetchone()[0] == 0


def test_selected_missing_input_and_invalid_complete_graph_never_register(workspace: Path) -> None:
    source(workspace, "orders.py", DECLARATION)
    source(
        workspace,
        "consumer.py",
        "from transflow import transform, Input, Output\n"
        '@transform(rows=Input("raw/orders"), output=Output("curated/orders"))\n'
        "def consumer(rows): return rows\n",
    )
    before = (workspace / ".transflow/catalog.toml").read_bytes()
    selected = plan_probe(workspace, "selected", "curated/orders")
    assert not selected["ok"], selected
    assert "head" in selected["error"] or "metadata" in selected["error"]
    source(
        workspace,
        "broken.py",
        "from transflow import transform, Input, Output\n"
        '@transform(rows=Input("unknown/typo"), output=Output("bad/output"))\n'
        "def broken(rows): return rows\n",
    )
    assert not plan_probe(workspace, "full", "raw/orders")["ok"]
    assert (workspace / ".transflow/catalog.toml").read_bytes() == before


@pytest.mark.parametrize("policy", ['"always"', '{"ttl_seconds": 60}', '"manual"'])
def test_source_policies_plan_new_sources_without_running_them(
    workspace: Path, policy: str
) -> None:
    source(
        workspace,
        "orders.py",
        f"from transflow import source_transform, Output\n"
        f'@source_transform(output=Output("raw/orders"), refresh={policy})\n'
        f'def orders(): raise AssertionError("source must not execute")\n',
    )
    source(
        workspace,
        "consumer.py",
        "from transflow import transform, Input, Output\n"
        '@transform(rows=Input("raw/orders"), output=Output("curated/orders"))\n'
        "def consumer(rows): return rows\n",
    )
    result = plan_probe(workspace, "full", "curated/orders")
    if policy == '"manual"':
        assert not result["ok"], result
        result = plan_probe(workspace, "full", "raw/orders")
        assert result["ok"], result
        assert (
            next(iter(result["result"]["plan"]["context"]["source_decisions"].values()))["reason"]
            == "Explicit"
        )
    else:
        assert result["ok"], result
        draft = result["result"]["plan"]
        assert len(draft["writes"]) == 2
        decision = next(iter(draft["context"]["source_decisions"].values()))
        assert decision["executes"]
        assert decision["reason"] == ("Always" if policy == '"always"' else "Missing")


@pytest.mark.parametrize("case", ["ready", "force", "never", "source", "pending"])
def test_accepted_cache_preparation_uses_frozen_context_and_actual_parents(
    workspace: Path, case: str
) -> None:
    declaration = DECLARATION
    operation = "cache-force" if case == "force" else "cache"
    target = "raw/orders"
    if case == "never":
        declaration = declaration.replace("@transform(output=", '@transform(cache="never", output=')
    elif case == "source":
        declaration = declaration.replace("transform", "source_transform")
    source(workspace, "orders.py", declaration)
    if case == "pending":
        operation = "cache-full"
        target = "curated/orders"
        source(
            workspace,
            "consumer.py",
            "from transflow import transform, Input, Output\n"
            '@transform(rows=Input("raw/orders"), output=Output("curated/orders"))\n'
            'def consumer(rows): raise AssertionError("must not execute")\n',
        )
    result = plan_probe(workspace, operation, target)
    assert result["ok"], result
    outcome = result["result"]
    if case == "ready":
        assert outcome["decision"] == "ready"
        assert not outcome["hit"]
    elif case == "pending":
        assert outcome["decision"] == "pending"
    else:
        assert outcome["decision"] == "execute"
        assert (
            outcome["reason"]
            == {"force": "forced", "never": "cache_never", "source": "source_refresh"}[case]
        )
    with closing(sqlite3.connect(workspace / ".transflow/runtime/catalog.sqlite")) as db:
        assert db.execute("SELECT count(*) FROM attempts").fetchone()[0] == 0
        assert db.execute("SELECT count(*) FROM dataset_versions").fetchone()[0] == 0
        assert db.execute("SELECT count(*) FROM cached_jobs").fetchone()[0] == 0
        assert db.execute("SELECT count(*) FROM publication_contracts").fetchone()[0] == (
            0 if case == "pending" else 1
        )
        assert db.execute("SELECT count(*) FROM computation_evidence").fetchone()[0] == (
            0 if case == "pending" else 1
        )


@pytest.mark.parametrize("registered", [False, True])
def test_freshness_inspection_keeps_unknowns_and_never_executes_or_registers(
    workspace: Path, registered: bool
) -> None:
    source(workspace, "orders.py", DECLARATION)
    if registered:
        cli(workspace, "catalog", "sync", "--python", sys.executable)
    registry = (workspace / ".transflow/catalog.toml").read_bytes()
    result = plan_probe(workspace, "why", "raw/orders")
    assert result["ok"], result
    status = result["result"]["datasets"][0]
    assert status["materialization"] == "NeverBuilt"
    assert status["direct_logic"] == "Unknown"
    codes = {r["code"] for r in status["reasons"]}
    assert "NEVER_BUILT" in codes
    assert ("CATALOG_ADDITION_PENDING" in codes) is not registered
    assert "CACHE_MATCH" not in codes
    assert (workspace / ".transflow/catalog.toml").read_bytes() == registry
    # Invalid current source cannot fall back to a previously discovered graph.
    source(workspace, "orders.py", "this is invalid Python !!!")
    assert not plan_probe(workspace, "why", "raw/orders")["ok"]
    with closing(sqlite3.connect(workspace / ".transflow/runtime/catalog.sqlite")) as db:
        assert db.execute("SELECT count(*) FROM attempts").fetchone()[0] == 0
        assert db.execute("SELECT count(*) FROM dataset_versions").fetchone()[0] == 0


@pytest.mark.parametrize("direction", ["upstream", "downstream"])
def test_captured_graph_traversal_depth_and_pagination_never_execute_producers(
    workspace: Path, direction: str
) -> None:
    source(workspace, "orders.py", DECLARATION)
    for name, inputs in [
        ("left", {"rows": "raw/orders"}),
        ("right", {"rows": "raw/orders"}),
        ("joined", {"left": "curated/left", "right": "curated/right"}),
    ]:
        bindings = ", ".join(f'{alias}=Input("{path}")' for alias, path in inputs.items())
        source(
            workspace,
            f"{name}.py",
            "from transflow import transform, Input, Output\n"
            f'@transform({bindings}, output=Output("curated/{name}"))\n'
            f"def produce({', '.join(inputs)}):\n"
            '    raise AssertionError("Traversal must not execute producers")\n',
        )
    start = "curated/joined" if direction == "upstream" else "raw/orders"
    registry = (workspace / ".transflow/catalog.toml").read_bytes()
    for depth, count, omitted in [("0", 1, 3), ("1", 3, 1), ("2", 4, 0), (None, 4, 0)]:
        result = plan_probe(workspace, direction, start, *([depth] if depth else []))
        assert result["ok"], result
        value = result["result"]
        assert len(value["nodes"]) == count
        assert len({n["identity"] for n in value["nodes"]}) == count
        assert value["nodes"][0]["paths"] == [start]
        assert value["nodes"][0]["depth"] == 0
        assert value["omitted_nodes"] == omitted
        assert value["scope_complete"] is (omitted == 0)
        assert value["branch"] == "feature"
        assert len(value["certificate"]) == 64
        if omitted == 0:
            assert len(value["edges"]) == 4
            assert value["pages"] == 4
    assert not plan_probe(workspace, direction, start, "-1")["ok"]
    assert (workspace / ".transflow/catalog.toml").read_bytes() == registry
    # A cycle anywhere in the source blocks even a depth-zero request for an unrelated node.
    source(
        workspace,
        "cycle.py",
        "from transflow import transform, Input, Output\n"
        '@transform(rows=Input("bad/cycle"), output=Output("bad/cycle"))\n'
        "def produce(rows): return rows\n",
    )
    assert not plan_probe(workspace, direction, start, "0")["ok"]
    assert not (workspace / ".transflow/runtime/catalog.sqlite").exists()


def test_typed_expectation_validation_never_runs_data_and_preserves_workspace(
    workspace: Path,
) -> None:
    code = """from transflow import Check, Output, transform
from transflow import expectations as E
positive = E.all(E.col("amount").gte(0), E.col("amount").lt(200))
@transform(output=Output(
    "raw/orders", checks=Check(E.all(E.col("amount").non_null(), positive), "Amount")
))
def orders():
    raise AssertionError("Validation must never execute the producer")
"""
    source(workspace, "orders.py", code)
    before = files(workspace)
    result = cli(workspace, "validate", "--python", sys.executable)
    assert result["exit_status"] == 0
    assert files(workspace) == before
    source(
        workspace, "orders.py", code.replace('E.col("amount").non_null()', 'E.primary_key("id")')
    )
    before = files(workspace)
    result = cli(workspace, "validate", "--python", sys.executable, ok=False)
    assert result["exit_status"] != 0
    assert files(workspace) == before
