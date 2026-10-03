"""Real persistent scheduling against an explicitly prepared, installed-worker workspace."""

from __future__ import annotations

import copy
import json
import sqlite3
import subprocess
import sys
import time
import tomllib
from contextlib import closing
from pathlib import Path
from typing import Any
from uuid import uuid4

from api_client import Client, coordinator


def exercise(cli: Path, root: Path, python: str) -> list[str]:
    cases: list[str] = []
    database = root / ".transflow/runtime/catalog.sqlite"
    local_config = root / "transflow.local.toml"
    local_text = local_config.read_text() if local_config.exists() else ""
    if "python" not in tomllib.loads(local_text):
        local_config.write_text(local_text + f"\n[python]\nexecutable = {json.dumps(python)}\n")

    def rows(sql: str, *args: object) -> list[tuple[Any, ...]]:
        with closing(sqlite3.connect(database)) as db:
            return db.execute(sql, args).fetchall()

    def command(*args: str) -> dict[str, Any]:
        p = subprocess.run(
            [cli, "--workspace", root, *args, "--json"],
            check=True,
            capture_output=True,
            text=True,
            timeout=180,
        )
        return json.loads(p.stdout)["result"]  # type: ignore[no-any-return]

    command("catalog", "sync", "--python", python)
    first = command("build", "raw/items", "--branch", "scheduled-base", "--python", python)
    build_id = first["data"]["id"]
    snapshot = rows(
        "SELECT p.source_snapshot_id FROM builds b JOIN build_plans p ON p.id=b.plan_id "
        "WHERE b.id=?",
        build_id,
    )[0][0]
    registry = tomllib.loads((root / ".transflow/catalog.toml").read_text())
    workspace = tomllib.loads((root / "workspace.toml").read_text())["workspace_id"]
    datasets = {d["path"]: d["id"] for d in registry["datasets"]}
    fixtures = json.loads(
        (Path(__file__).parents[2] / "schemas/fixtures/conformance.json").read_text()
    )
    template = next(c["value"] for c in fixtures if c["name"] == "schedule-definition-fixed")

    def definition(name: str, target: str = "raw/items") -> dict[str, Any]:
        value: dict[str, Any] = copy.deepcopy(template)
        value["name"] = name
        value["build"].update(
            source={"kind": "fixed_snapshot", "snapshot_id": snapshot},
            data_branch="scheduled-out",
            build_mode="selected",
            targets=[{"workspace_id": workspace, "dataset_id": datasets[target]}],
            boundaries=[],
            exclusions=[],
            refresh_sources=[],
            parameters={},
            force=True,
            require_current=False,
            fallback_branches=[],
            input_fallback_policy={"default": [], "rules": {}},
            provider_fallback_policies={},
        )
        value["trigger"] = {"kind": "manual"}
        value["policies"].update(minimum_delay_seconds=1, max_consecutive_builds=3)
        return value

    def post(client: Client, path: str, body: object, etag: str | None = None) -> dict[str, Any]:
        headers = {"Idempotency-Key": str(uuid4())}
        if etag:
            headers["If-Match"] = f'"{etag}"'
        deadline = time.monotonic() + 30
        while True:
            result = client.call("POST", path, body, headers=headers, status=None)
            if result.get("error", {}).get("code") == "TF_API_BUSY":
                assert time.monotonic() < deadline, result
                time.sleep(0.02)
                continue
            assert "error" not in result, result
            return result["data"]  # type: ignore[no-any-return]

    def save(client: Client, value: dict[str, Any], paused: bool = True) -> tuple[str, str]:
        schedule = str(uuid4())
        saved = post(
            client, "/api/v1/schedules", {"id": schedule, "paused": paused, "definition": value}
        )
        return schedule, str(saved["etag"])

    def wait(schedule: str, count: int = 1) -> list[tuple[Any, ...]]:
        deadline = time.monotonic() + 60
        while True:
            states = rows(
                "SELECT disposition,build_id FROM schedule_occurrences WHERE schedule_id=? "
                "ORDER BY logical_fire_at_us,id",
                schedule,
            )
            if len(states) >= count and all(
                s[0] in {"SUCCEEDED", "FAILED", "CANCELED"} for s in states
            ):
                return states
            assert time.monotonic() < deadline, (
                states,
                rows(
                    "SELECT operation,evidence_json FROM audit_log ORDER BY sequence DESC LIMIT 5"
                ),
            )
            time.sleep(0.02)

    items = root / "src/items.py"
    original = items.read_text()
    catalog = (root / ".transflow/catalog.toml").read_bytes()
    with coordinator(root, cli) as registration:
        client = Client(registration)
        schedule, etag = save(client, definition("retained non-Git"))
        items.write_text("raise RuntimeError('foreground source must not be imported')\n")
        post(client, f"/api/v1/schedules/{schedule}/run", {}, etag)
        finished = wait(schedule)
        assert finished[0][0] == "SUCCEEDED", (
            finished,
            rows("SELECT operation,evidence_json FROM audit_log ORDER BY sequence DESC LIMIT 5"),
        )
        assert (root / ".transflow/catalog.toml").read_bytes() == catalog
        assert rows(
            "SELECT count(*) FROM jobs j JOIN data_branches b ON b.id=j.branch_id "
            "WHERE j.build_id=? AND b.name='scheduled-out'",
            finished[0][1],
        ) == [(1,)]
        assert rows(
            "SELECT count(*) FROM events WHERE type='schedule.succeeded' "
            "AND json_extract(payload_json,'$.occurrence') "
            "IN (SELECT id FROM schedule_occurrences WHERE schedule_id=?)",
            schedule,
        ) == [(1,)]
        items.write_text(original)
        cases.append(
            "non-Git retained source executes while paused without importing or editing live source"
        )
    with coordinator(root, cli):
        assert rows(
            "SELECT count(*) FROM builds WHERE occurrence_id "
            "IN (SELECT id FROM schedule_occurrences WHERE schedule_id=?)",
            schedule,
        ) == [(1,)]
    cases.append(
        "restart preserves the committed schedule success and does not resubmit its occurrence"
    )
    with coordinator(root, cli) as registration:
        client = Client(registration)
        cached = definition("scheduled cache reuse")
        cached["build"]["force"] = False
        schedule, etag = save(client, cached)
        post(client, f"/api/v1/schedules/{schedule}/run", {}, etag)
        finished = wait(schedule)
        assert finished[0][0] == "SUCCEEDED", finished
        assert rows(
            "SELECT count(*) FROM attempts a JOIN jobs j ON j.id=a.job_id WHERE j.build_id=?",
            finished[0][1],
        ) == [(0,)]
    cases.append("unforced scheduled cache reuse succeeds without launching another producer")

    # A real dataset publication on another branch becomes an exact scheduled input.
    with coordinator(root, cli) as registration:
        client = Client(registration)
        automatic = definition("event pin", "curated/left")
        automatic["trigger"] = {
            "kind": "dataset_published",
            "id": "published",
            "dataset": {"workspace_id": workspace, "dataset_id": datasets["raw/items"]},
            "branch": "scheduled-base",
            "payload_mode": "pin",
            "include_resets": False,
        }
        schedule, _ = save(client, automatic, paused=False)
        command("build", "raw/items", "--branch", "scheduled-base", "--python", python, "--force")
        finished = wait(schedule)
        assert finished[0][0] == "SUCCEEDED", finished
        payload = json.loads(
            rows("SELECT payload_json FROM schedule_occurrences WHERE schedule_id=?", schedule)[0][
                0
            ]
        )
        pinned = payload["input_pins"][0]["version_id"]
        assert rows(
            "SELECT i.version_id FROM job_inputs i JOIN jobs j ON j.id=i.job_id WHERE j.build_id=?",
            finished[0][1],
        ) == [(pinned,)]
        assert rows("SELECT count(*) FROM jobs WHERE build_id=?", finished[0][1]) == [(1,)]
        cases.append(
            "real committed event dispatches a boundary pin across branches "
            "without rebuilding its input"
        )
        # Preserve an already accepted event pin while the provider head advances before dispatch.
        queued = definition("frozen pending input", "curated/left")
        queued_schedule, _ = save(client, queued)
    with closing(sqlite3.connect(database)) as db:
        occurrence = str(uuid4())
        frozen = {"build": queued["build"], "policies": queued["policies"]}
        payload["tokens"] = []
        db.execute(
            "INSERT INTO schedule_occurrences(id,schedule_id,trigger_epoch,evidence_digest,"
            "logical_fire_at_us,payload_json,execution_json,disposition) "
            "SELECT ?,id,trigger_epoch,?, ?,?,?,'QUEUED' FROM schedules WHERE id=?",
            (
                occurrence,
                str(uuid4()),
                time.time_ns() // 1000,
                json.dumps(payload),
                json.dumps(frozen),
                queued_schedule,
            ),
        )
        db.commit()
    command("build", "raw/items", "--branch", "scheduled-base", "--python", python, "--force")
    with closing(sqlite3.connect(database)) as db:
        db.execute("UPDATE schedules SET paused=0 WHERE id=?", (queued_schedule,))
        db.commit()
    with coordinator(root, cli):
        finished = wait(queued_schedule)
        assert finished[0][0] == "SUCCEEDED", finished
        assert rows(
            "SELECT i.version_id FROM job_inputs i JOIN jobs j ON j.id=i.job_id WHERE j.build_id=?",
            finished[0][1],
        ) == [(pinned,)]
        latest = rows(
            "SELECT h.version_id FROM dataset_heads h JOIN data_branches b ON b.id=h.branch_id "
            "WHERE h.dataset_id=? AND b.name='scheduled-base'",
            datasets["raw/items"],
        )[0][0]
        assert latest != pinned
    cases.append(
        "queued automatic occurrence retains its original exact version "
        "after head movement and restart"
    )
    with coordinator(root, cli) as registration:
        client = Client(registration)
        cron = definition("restart cron observation")
        cron["trigger"] = {
            "kind": "cron",
            "id": "minute",
            "expression": "* * * * *",
            "timezone": "UTC",
            "duplicate_time": "earliest",
        }
        cron["policies"]["misfire_policy"] = "coalesce_latest"
        cron_schedule, _ = save(client, cron, paused=False)
    # A durable observation cursor from before downtime exercises the real system clock on restart.
    with closing(sqlite3.connect(database)) as db:
        cursor = time.time_ns() // 1000 - 61_000_000
        db.execute("UPDATE schedules SET saved_at_us=? WHERE id=?", (cursor, cron_schedule))
        db.execute(
            "INSERT INTO schedule_clock_state(schedule_id,trigger_epoch,cursor_at_us,cursor_leaf,"
            "matched_count,missed_count,ignored_count,coalesced_count) "
            "SELECT id,trigger_epoch,?,63,0,0,0,0 FROM schedules WHERE id=? "
            "ON CONFLICT(schedule_id) DO UPDATE "
            "SET cursor_at_us=excluded.cursor_at_us,cursor_leaf=63",
            (cursor, cron_schedule),
        )
        db.commit()
    with coordinator(root, cli) as registration:
        finished = wait(cron_schedule)
        assert finished[0][0] == "SUCCEEDED", finished
        clock_payload = json.loads(
            rows(
                "SELECT payload_json FROM schedule_occurrences WHERE schedule_id=? "
                "ORDER BY logical_fire_at_us LIMIT 1",
                cron_schedule,
            )[0][0]
        )
        assert clock_payload["tokens"][0]["event_kind"] == "schedule.tick"
        post(
            Client(registration),
            f"/api/v1/schedules/{cron_schedule}/pause",
            {},
            rows("SELECT etag FROM schedules WHERE id=?", cron_schedule)[0][0],
        )
    cases.append("restart observes a persisted missed cron tick and dispatches a real worker job")

    def git(*args: str) -> str:
        result = subprocess.run(
            [
                "git",
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                *args,
            ],
            cwd=root,
            check=True,
            capture_output=True,
            text=True,
            timeout=30,
        )
        return result.stdout.strip()

    git("init", "-b", "master")
    git(
        "add",
        "workspace.toml",
        ".transflow/catalog.toml",
        "requirements.in",
        "requirements.lock",
        "src",
    )
    git("commit", "-m", "registered source fixture")
    selected_commit = git("rev-parse", "HEAD")
    with coordinator(root, cli) as registration:
        client = Client(registration)
        value = definition("clean ref")
        value["build"]["source"] = {"kind": "git_ref", "ref": "refs/heads/master"}
        schedule, etag = save(client, value)
        git("checkout", "-b", "foreground")
        items.write_text("raise RuntimeError('dirty foreground source must not be imported')\n")
        status = git("status", "--porcelain")
        post(client, f"/api/v1/schedules/{schedule}/run", {}, etag)
        finished = wait(schedule)
        assert finished[0][0] == "SUCCEEDED", finished
        assert git("rev-parse", "HEAD") == selected_commit
        assert git("branch", "--show-current") == "foreground"
        assert git("status", "--porcelain") == status
        assert rows(
            "SELECT DISTINCT b.name FROM jobs j JOIN data_branches b ON b.id=j.branch_id "
            "WHERE j.build_id=?",
            finished[0][1],
        ) == [("scheduled-out",)]
        cases.append(
            "saved Git ref and output branch ignore later foreground branch and dirty source"
        )
        # Working-tree imports are mandatory; invalid source becomes an explicit failed occurrence.
        items.write_text(original)
        invalid = definition("invalid changed source")
        invalid["build"]["source"] = {"kind": "working_tree", "allow_additive_sync": False}
        schedule, etag = save(client, invalid)
        items.write_text("invalid Python !!!\n")
        before = rows("SELECT count(*) FROM attempts")
        post(client, f"/api/v1/schedules/{schedule}/run", {}, etag)
        failed = wait(schedule)
        assert failed == [("FAILED", None)], failed
        assert rows("SELECT count(*) FROM attempts") == before
        assert (root / ".transflow/catalog.toml").read_bytes() == catalog
        cases.append("invalid changed working-tree source fails before any job or publication")
        # An unregistered output in the saved ref must not create a live-checkout identity.
        items.write_text(original)
        (root / "src/unregistered.py").write_text(
            'from transflow import transform, Output\n@transform(output=Output("new/scheduled"))\n'
            'def unregistered(): raise AssertionError("must not execute")\n'
        )
        git("add", "src/unregistered.py")
        git("commit", "-m", "unregistered source fixture")
        unregistered = definition("unregistered ref")
        unregistered["build"]["source"] = {"kind": "git_ref", "ref": "refs/heads/foreground"}
        schedule, etag = save(client, unregistered)
        git("checkout", "master")
        post(client, f"/api/v1/schedules/{schedule}/run", {}, etag)
        assert wait(schedule) == [("FAILED", None)]
        assert (root / ".transflow/catalog.toml").read_bytes() == catalog
        assert rows("SELECT count(*) FROM attempts") == before
        cases.append("unregistered ref fails without assigning IDs or mutating another checkout")
        items.write_text(original)
        (root / "src/new_output.py").write_text(
            'from transflow import transform, Output\n@transform(output=Output("new/allowed"))\n'
            'def extra(): raise AssertionError("unselected producer must not execute")\n'
        )
        working = definition("working-tree registration denied")
        working["build"]["source"] = {"kind": "working_tree", "allow_additive_sync": False}
        schedule, etag = save(client, working)
        post(client, f"/api/v1/schedules/{schedule}/run", {}, etag)
        assert wait(schedule) == [("FAILED", None)]
        assert (root / ".transflow/catalog.toml").read_bytes() == catalog
        cases.append(
            "working-tree source refuses unregistered IDs unless additive sync is explicit"
        )
        working["build"]["source"]["allow_additive_sync"] = True
        working["name"] = "working-tree registration allowed"
        schedule, etag = save(client, working)
        post(client, f"/api/v1/schedules/{schedule}/run", {}, etag)
        assert wait(schedule)[0][0] == "SUCCEEDED"
        registered = tomllib.loads((root / ".transflow/catalog.toml").read_text())["datasets"]
        assert any(d["path"] == "new/allowed" for d in registered)
        cases.append("explicit working-tree additive sync registers IDs before selected execution")
    return cases


def exercise_foreign(
    cli: Path, consumer: Path, provider: Path, python: str, build_id: str
) -> list[str]:
    """Frozen provider policy and exact foreign pins through the installed worker."""
    database = consumer / ".transflow/runtime/catalog.sqlite"
    provider_config = provider / "workspace.toml"
    original = provider_config.read_text()
    provider_id = tomllib.loads(original)["workspace_id"]
    local_config = consumer / "transflow.local.toml"
    local_config.write_text(
        local_config.read_text() + f"\n[python]\nexecutable = {json.dumps(python)}\n"
    )

    def rows(sql: str, *args: object) -> list[tuple[Any, ...]]:
        with closing(sqlite3.connect(database)) as db:
            return db.execute(sql, args).fetchall()

    snapshot = rows(
        "SELECT p.source_snapshot_id FROM builds b JOIN build_plans p ON p.id=b.plan_id "
        "WHERE b.id=?",
        build_id,
    )[0][0]
    workspace = tomllib.loads((consumer / "workspace.toml").read_text())["workspace_id"]
    dataset = tomllib.loads((consumer / ".transflow/catalog.toml").read_text())["datasets"][0]["id"]
    observed = rows("SELECT workspace_id,dataset_id,version_id FROM foreign_versions")[0]
    fixtures = json.loads(
        (Path(__file__).parents[2] / "schemas/fixtures/conformance.json").read_text()
    )
    value = copy.deepcopy(
        next(c["value"] for c in fixtures if c["name"] == "schedule-definition-fixed")
    )
    value["trigger"] = {"kind": "manual"}
    value["build"].update(
        source={"kind": "fixed_snapshot", "snapshot_id": snapshot},
        data_branch="scheduled-external",
        build_mode="selected",
        targets=[{"workspace_id": workspace, "dataset_id": dataset}],
        boundaries=[],
        exclusions=[],
        refresh_sources=[],
        parameters={},
        force=True,
        require_current=False,
        fallback_branches=[],
        input_fallback_policy={"default": [], "rules": {}},
        provider_fallback_policies={provider_id: {"default": ["master"], "rules": {}}},
    )
    value["policies"].update(minimum_delay_seconds=1, max_consecutive_builds=3)

    def post(client: Client, path: str, body: object, etag: str | None = None) -> dict[str, Any]:
        headers = {"Idempotency-Key": str(uuid4())}
        if etag:
            headers["If-Match"] = f'"{etag}"'
        deadline = time.monotonic() + 30
        while True:
            result = client.call("POST", path, body, headers=headers, status=None)
            if result.get("error", {}).get("code") == "TF_API_BUSY":
                assert time.monotonic() < deadline, result
                time.sleep(0.02)
                continue
            assert "error" not in result, result
            return result["data"]  # type: ignore[no-any-return]

    def wait(schedule: str) -> str:
        deadline = time.monotonic() + 60
        while True:
            states = rows(
                "SELECT disposition,build_id FROM schedule_occurrences WHERE schedule_id=?",
                schedule,
            )
            if states and states[0][0] in {"SUCCEEDED", "FAILED"}:
                assert states[0][0] == "SUCCEEDED", (states, rows("SELECT * FROM audit_log"))
                return str(states[0][1])
            assert time.monotonic() < deadline, states
            time.sleep(0.02)

    try:
        with coordinator(consumer, cli) as registration:
            client = Client(registration)
            schedule = str(uuid4())
            saved = post(
                client, "/api/v1/schedules", {"id": schedule, "paused": True, "definition": value}
            )
            assert "[branching]" not in original
            provider_config.write_text(original + "\n[branching]\ndefault_fallbacks = []\n")
            post(client, f"/api/v1/schedules/{schedule}/run", {}, saved["etag"])
            scheduled_build = wait(schedule)
            assert rows(
                "SELECT foreign_workspace_id,foreign_version_id FROM job_inputs i "
                "JOIN jobs j ON j.id=i.job_id "
                "WHERE j.build_id=?",
                scheduled_build,
            ) == [(provider_id, observed[2])]
            # A T092-shaped accepted foreign occurrence bypasses moving fallback resolution.
            value["build"]["provider_fallback_policies"][provider_id]["default"] = []
            value["name"] = "scheduled foreign exact pin"
            pinned_schedule = str(uuid4())
            post(
                client,
                "/api/v1/schedules",
                {"id": pinned_schedule, "paused": True, "definition": value},
            )
        pin = dict(
            zip(
                ["workspace_id", "dataset_id", "version_id"],
                observed,
                strict=True,
            )
        )
        pin["branch"] = "master"
        pin["generation"] = "1"
        pin["cause"] = "publication"
        with closing(sqlite3.connect(database)) as db:
            db.execute(
                "INSERT INTO schedule_occurrences(id,schedule_id,trigger_epoch,evidence_digest,"
                "logical_fire_at_us,payload_json,execution_json,disposition) "
                "SELECT ?,id,trigger_epoch,?,?,?,?,'QUEUED' FROM schedules WHERE id=?",
                (
                    str(uuid4()),
                    str(uuid4()),
                    time.time_ns() // 1000,
                    json.dumps({"tokens": [], "input_pins": [pin]}),
                    json.dumps({"build": value["build"], "policies": value["policies"]}),
                    pinned_schedule,
                ),
            )
            db.execute("UPDATE schedules SET paused=0 WHERE id=?", (pinned_schedule,))
            db.commit()
        with coordinator(consumer, cli):
            pinned_build = wait(pinned_schedule)
            assert rows(
                "SELECT foreign_workspace_id,foreign_version_id FROM job_inputs i "
                "JOIN jobs j ON j.id=i.job_id "
                "WHERE j.build_id=?",
                pinned_build,
            ) == [(provider_id, observed[2])]
            assert rows("SELECT count(*) FROM replicas") == [(0,)]
        return [
            "scheduled foreign read freezes provider fallbacks despite later policy edits",
            "queued foreign exact pin reads provider bytes without local replication",
        ]
    finally:
        provider_config.write_text(original)


if __name__ == "__main__":
    print(json.dumps({"cases": exercise(Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3])}))
