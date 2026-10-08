"""Public schedule CLI/API agreement, snapshot replacement and explicit export/delete."""

from __future__ import annotations

import copy
import json
import sqlite3
import sys
import time
import tomllib
from contextlib import closing
from pathlib import Path
from uuid import uuid4

from api_client import Client, coordinator
from test_catalog_cli import CLI, DECLARATION, cli, source
from test_catalog_cli import workspace as workspace
from test_packaging import REPOSITORY
from test_packaging import wheel as wheel
from transflow_worker.wire import validate_document


def definition(root: Path) -> dict[str, object]:
    fixtures = json.loads((REPOSITORY / "schemas/fixtures/conformance.json").read_text())
    value = copy.deepcopy(
        next(case["value"] for case in fixtures if case["name"] == "schedule-definition-fixed")
    )
    registry = tomllib.loads((root / ".transflow/catalog.toml").read_text())
    workspace_id = tomllib.loads((root / "workspace.toml").read_text())["workspace_id"]
    value["name"] = "CLI review schedule"
    value["build"].update(
        source={"kind": "working_tree", "allow_additive_sync": False},
        data_branch="master",
        targets=[{"workspace_id": workspace_id, "dataset_id": registry["datasets"][0]["id"]}],
        boundaries=[],
        exclusions=[],
        refresh_sources=[],
        parameters={},
        fallback_branches=[],
        input_fallback_policy={"default": [], "rules": {}},
    )
    build = value["build"]
    assert isinstance(build, dict)
    value["trigger"] = {"kind": "manual"}
    return value  # type: ignore[no-any-return]


def test_schedule_cli_snapshot_export_guarded_actions_and_retained_history(workspace: Path) -> None:
    source(workspace, "orders.py", DECLARATION)
    cli(workspace, "catalog", "sync", "--python", sys.executable)
    (workspace / "transflow.local.toml").write_text(
        f"[python]\nexecutable={json.dumps(sys.executable)}\n"
    )
    path = workspace / "schedule.json"
    value = definition(workspace)
    build = value["build"]
    assert isinstance(build, dict)
    value["trigger"] = {
        "kind": "dataset_published",
        "id": "input-published",
        "dataset": build["targets"][0],
        "branch": "input",
        "payload_mode": "signal_only",
        "include_resets": False,
    }
    path.write_text(json.dumps(value))
    created = cli(workspace, "schedule", "create", "--file", str(path), "--paused")["result"]
    assert created["operation"] == "create"
    schedule = created["data"]["id"]
    etag = created["data"]["etag"]
    listed = cli(workspace, "schedule", "list")["result"]["data"]
    assert [row["id"] for row in listed["schedules"]] == [schedule]
    exported = workspace / "schedule-export.json"
    cli(workspace, "schedule", "export", schedule, "--path", str(exported))
    assert json.loads(exported.read_text()) == value
    cli(workspace, "schedule", "export", schedule, "--path", str(exported), ok=False)
    cli(workspace, "schedule", "run", schedule, ok=False)
    with coordinator(workspace, CLI) as registration:
        client = Client(registration)
        assert client.call("GET", f"/api/v1/schedules/{schedule}")["data"]["definition"] == value
        context = client.call("GET", "/api/v1/context", query={"branch": "master"})["context"]
        defaults = client.call(
            "GET",
            "/api/v1/schedules/defaults",
            query={"branch": "master", "context": context["fingerprint"]},
        )["data"]
        validate_document("ScheduleDefinitionV1", defaults)
        assert defaults["build"]["source"]["kind"] == "fixed_snapshot"
        roles = client.call(
            "GET",
            f"/api/v1/schedules/{schedule}/roles",
            query={"branch": "master", "context": context["fingerprint"]},
        )["data"]
        validate_document("ApiScheduleRolesV1", roles)
        assert roles["nodes"][0]["roles"] == ["target", "trigger"]
        clock_definition = copy.deepcopy(value)
        clock_definition["trigger"] = {
            "kind": "and",
            "children": [
                {
                    "kind": "cron",
                    "id": "morning",
                    "expression": "0 9 * * 1-5",
                    "timezone": "Asia/Dubai",
                    "duplicate_time": "earliest",
                },
                value["trigger"],
            ],
        }
        clock = client.call("POST", "/api/v1/schedules/clock-preview", clock_definition)["data"]
        validate_document("ApiScheduleClockPreviewV1", clock)
        assert len(clock["leaves"][0]["fires"]) == 5
        assert all(f["local"].endswith("+04:00") for f in clock["leaves"][0]["fires"])
        preview = client.call(
            "POST",
            "/api/v1/schedules/preview",
            value,
            query={"branch": "master", "context": context["fingerprint"]},
            headers={"If-Match": f'"{context["fingerprint"]}"', "Idempotency-Key": str(uuid4())},
        )["data"]
        validate_document("PlanResultV1", preview)
        assert preview["producer_execution"] is False
        assert preview["writes"]
        rejected = cli(workspace, "build", "--plan", preview["plan_id"], ok=False)
        assert any(
            "Schedule scope previews cannot be accepted" in d["reason"]
            for d in rejected["diagnostics"]
        ), rejected
        run = cli(workspace, "schedule", "run", schedule)["result"]["data"]
        validate_document("ScheduleRunV1", run)
        assert client.call("GET", f"/api/v1/schedules/{schedule}")["data"]["paused"]
        database = workspace / ".transflow/runtime/catalog.sqlite"
        deadline = time.monotonic() + 60
        while True:
            with closing(sqlite3.connect(database)) as db:
                state = db.execute(
                    "SELECT disposition FROM schedule_occurrences WHERE id=?", (run["id"],)
                ).fetchone()
            if state and state[0] in {"SUCCEEDED", "FAILED"}:
                break
            assert time.monotonic() < deadline, state
            time.sleep(0.02)
        history = cli(workspace, "schedule", "history", schedule)["result"]["data"]
        validate_document("ApiScheduleHistoryV1", history)
        assert [row["id"] for row in history["occurrences"]] == [run["id"]]
        assert client.call("GET", f"/api/v1/schedules/{schedule}/history")["data"] == history
        now = time.time_ns() // 1000 + 1_000_000
        metrics = cli(
            workspace, "schedule", "metrics", schedule, "--from-us", "0", "--to-us", str(now)
        )["result"]["data"]
        validate_document("ApiScheduleMetricsV1", metrics)
        assert metrics["occurrences"] == "1"
        value["description"] = "Overwrite the current snapshot"
        path.write_text(json.dumps(value))
        updated = cli(
            workspace, "schedule", "update", schedule, "--file", str(path), "--if-match", etag
        )["result"]["data"]
        assert updated["definition"]["description"] == value["description"]
        cli(
            workspace,
            "schedule",
            "update",
            schedule,
            "--file",
            str(path),
            "--if-match",
            etag,
            ok=False,
        )
        paused = cli(workspace, "schedule", "pause", schedule)["result"]["data"]
        assert paused["paused"]
        resumed = cli(workspace, "schedule", "resume", schedule)["result"]["data"]
        assert not resumed["paused"]
        preview = cli(workspace, "schedule", "delete", schedule)["result"]["data"]
        assert preview["preview"] and not preview["deleted"]
        removed = cli(workspace, "schedule", "delete", schedule, "--yes")["result"]["data"]
        assert removed["deleted"] and removed["retained_history"]
        assert not cli(workspace, "schedule", "list")["result"]["data"]["schedules"]
        assert cli(workspace, "schedule", "history", schedule)["result"]["data"]["occurrences"]
