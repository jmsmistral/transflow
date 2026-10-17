"""Explicit public dry run/apply against real imported immutable Parquet objects."""

import copy
import json
import sqlite3
import sys
import time
from contextlib import closing
from pathlib import Path

from api_client import coordinator
from test_catalog_cli import CLI, cli, files, parquet_input
from test_catalog_cli import workspace as workspace
from test_packaging import wheel as wheel
from transflow_worker.canonical import artifact_digest, canonical_json


def stable_files(root: Path) -> dict[str, str]:
    # SQLite may maintain its transient WAL index even for read-only connections.
    return {p: h for p, h in files(root).items() if not p.endswith(("-shm", "-wal"))}


def test_gc_dry_run_is_read_only_and_apply_observes_verified_orphans(workspace: Path) -> None:
    path = parquet_input(workspace)
    result = cli(
        workspace, "dataset", "import", "raw/input", "--path", str(path), "--python", sys.executable
    )["result"]
    # Two explicit captures leave a verified old descriptor snapshot outside the
    # current graph, while the imported version's original capture stays rooted.
    cli(workspace, "catalog", "sync", "--python", sys.executable)
    cli(workspace, "catalog", "sync", "--python", sys.executable)
    objects = workspace / ".transflow/runtime/objects"
    original = next(objects.glob("??/*/manifest.json"))
    manifest = copy.deepcopy(json.loads(original.read_text()))
    manifest["writer"]["version"] = "gc-orphan"
    digest = artifact_digest(manifest).hex
    orphan = objects / digest[:2] / digest
    orphan.mkdir(parents=True)
    (orphan / "manifest.json").write_bytes(canonical_json(manifest))
    for file in manifest["files"]:
        (orphan / file["path"]).write_bytes((original.parent / file["path"]).read_bytes())
    unknown = objects / ".unknown-staging"
    unknown.mkdir()
    (unknown / "preserve").write_text("unknown state")
    before = stable_files(workspace)
    report = cli(workspace, "gc")["result"]
    assert report["kind"] == "gc" and report["applied"] is False
    assert any(i["id"] == digest and i["action"] == "observe" for i in report["items"])
    assert any(i["kind"] == "unknown" and i["action"] == "retain" for i in report["items"])
    assert stable_files(workspace) == before
    rejected = cli(workspace, "gc", "--apply", ok=False)
    assert "--yes" in rejected["diagnostics"][0]["reason"]
    applied = cli(workspace, "gc", "--apply", "--yes")["result"]
    assert applied["applied"] is True and applied["deleted_bytes"] == "0"
    assert any(i["id"] == digest and i["action"] == "observed" for i in applied["items"])
    again = cli(workspace, "gc", "--apply", "--yes")["result"]
    assert again["deleted_bytes"] == "0" and orphan.exists() and unknown.exists()
    with closing(sqlite3.connect(workspace / ".transflow/runtime/catalog.sqlite")) as db:
        assert (
            db.execute("SELECT version_id FROM dataset_heads").fetchone()[0] == result["version_id"]
        )
        assert db.execute("SELECT count(*) FROM gc_deletions").fetchone()[0] == 0
        assert db.execute("SELECT count(*) FROM gc_items WHERE state='OBSERVED'").fetchone()[0] >= 1
        observed_sources = [
            row[0]
            for row in db.execute(
                "SELECT object_key FROM gc_items WHERE kind='source' AND state='OBSERVED'"
            )
        ]
        assert observed_sources
        # Controlled first-seen clock fixture, not filesystem timestamp authority.
        db.execute(
            "UPDATE gc_items SET first_seen_us=? WHERE state='OBSERVED'",
            (time.time_ns() // 1000 - 8 * 24 * 60 * 60 * 1_000_000,),
        )
        db.commit()
    collected = cli(workspace, "gc", "--apply", "--yes")["result"]
    assert int(collected["deleted_bytes"]) > 0
    assert not orphan.exists() and unknown.exists() and original.exists()
    for key in observed_sources:
        assert not (workspace / ".transflow/runtime/source-snapshots" / key).exists()
    with closing(sqlite3.connect(workspace / ".transflow/runtime/catalog.sqlite")) as db:
        assert db.execute("SELECT count(*) FROM gc_deletions").fetchone()[0] >= 2
        assert (
            db.execute("SELECT version_id FROM dataset_heads").fetchone()[0] == result["version_id"]
        )


def test_gc_apply_refuses_running_coordinator_and_symlink(workspace: Path) -> None:
    path = parquet_input(workspace)
    cli(
        workspace, "dataset", "import", "raw/input", "--path", str(path), "--python", sys.executable
    )
    with coordinator(workspace, CLI):
        report = cli(workspace, "gc")["result"]
        assert report["applied"] is False
        failed = cli(workspace, "gc", "--apply", "--yes", ok=False)
        assert failed["outcome"] == "failure"
    source = workspace / ".transflow/runtime/source-snapshots"
    (source / "unexpected-link").symlink_to(workspace.parent)
    # Unknown names are retained; a canonical-looking capture symlink is refused.
    (source / "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").symlink_to(workspace.parent)
    assert cli(workspace, "gc", ok=False)["outcome"] == "failure"
    assert workspace.parent.exists()
