"""Observable installed-worker concurrency and reservation qualification for T096."""

from __future__ import annotations

import copy
import json
import sqlite3
import subprocess
import time
import tomllib
from contextlib import closing
from pathlib import Path
from typing import Any
from uuid import uuid4

from api_client import Client, coordinator


def exercise(cli: Path, root: Path, python: str) -> list[str]:
    database = root / ".transflow/runtime/catalog.sqlite"
    gates = root / ".transflow/runtime/overlap-gates"
    gates.mkdir()

    def rows(sql: str, *args: object) -> list[tuple[Any, ...]]:
        with closing(sqlite3.connect(database)) as db:
            return db.execute(sql, args).fetchall()

    def wait(predicate: Any) -> Any:
        deadline = time.monotonic() + 90
        while True:
            value = predicate()
            if value:
                return value
            assert time.monotonic() < deadline, rows(
                "SELECT disposition,build_id FROM schedule_occurrences"
            )
            time.sleep(0.02)

    def command(*args: str) -> dict[str, Any]:
        process = subprocess.run(
            [cli, "--workspace", root, *args, "--json"],
            check=False,
            capture_output=True,
            text=True,
            timeout=180,
        )
        assert process.returncode == 0, (process.stdout, process.stderr)
        return json.loads(process.stdout)["result"]  # type: ignore[no-any-return]

    for name in ("slow", "other"):
        code = f'''import time
from pathlib import Path
import polars as pl
from transflow import transform, Output
@transform(output=Output("ops/{name}"))
def {name}():
    gate = Path({json.dumps(str(gates))})
    (gate / "{name}-started").touch()
    deadline = time.monotonic() + 90
    while not (gate / "{name}-release").exists():
        if time.monotonic() > deadline:
            raise RuntimeError("The synthetic execution gate was not released")
        time.sleep(0.02)
    return pl.DataFrame({{"id": [1, 2]}}).lazy()
'''
        (root / f"src/overlap_{name}.py").write_text(code)
        (gates / f"{name}-release").touch()
    retry_code = f"""import os
from pathlib import Path
import polars as pl
from transflow import transform, Output
@transform(output=Output("ops/retry"))
def retry():
    marker = Path({json.dumps(str(gates / "crashed-once"))})
    if not marker.exists():
        marker.touch()
        os._exit(17)
    return pl.DataFrame({{"id": [3]}}).lazy()
"""
    (root / "src/overlap_retry.py").write_text(retry_code)
    command("catalog", "sync", "--python", python)
    first = command("build", "ops/slow", "--branch", "overlap", "--python", python, "--force")
    command("build", "ops/other", "--branch", "overlap", "--python", python, "--force")
    snapshot = rows(
        "SELECT p.source_snapshot_id FROM builds b JOIN build_plans p ON p.id=b.plan_id "
        "WHERE b.id=?",
        first["data"]["id"],
    )[0][0]
    workspace = tomllib.loads((root / "workspace.toml").read_text())["workspace_id"]
    datasets = {
        d["path"]: d["id"]
        for d in tomllib.loads((root / ".transflow/catalog.toml").read_text())["datasets"]
    }
    fixtures = json.loads(
        (Path(__file__).parents[2] / "schemas/fixtures/conformance.json").read_text()
    )
    template = next(
        case["value"] for case in fixtures if case["name"] == "schedule-definition-fixed"
    )

    def definition(name: str, target: str) -> dict[str, Any]:
        value: dict[str, Any] = copy.deepcopy(template)
        value["name"] = name
        value["trigger"] = {"kind": "manual"}
        value["build"].update(
            source={"kind": "fixed_snapshot", "snapshot_id": snapshot},
            data_branch="overlap",
            targets=[{"workspace_id": workspace, "dataset_id": datasets[target]}],
            build_mode="selected",
            boundaries=[],
            exclusions=[],
            refresh_sources=[],
            parameters={},
            force=True,
            fallback_branches=[],
            input_fallback_policy={"default": [], "rules": {}},
            provider_fallback_policies={},
        )
        return value

    def post(
        client: Client, path: str, body: object, etag: str | None = None, method: str = "POST"
    ) -> dict[str, Any]:
        headers = {"Idempotency-Key": str(uuid4())}
        if etag:
            headers["If-Match"] = f'"{etag}"'
        deadline = time.monotonic() + 60
        while True:
            result = client.call(method, path, body, headers=headers, status=None)
            if result.get("error", {}).get("code") == "TF_API_BUSY":
                assert time.monotonic() < deadline, result
                time.sleep(0.02)
                continue
            assert "error" not in result, result
            return result["data"]  # type: ignore[no-any-return]

    def state(schedule: str) -> list[tuple[Any, ...]]:
        return rows(
            "SELECT disposition,build_id FROM schedule_occurrences WHERE schedule_id=? "
            "ORDER BY logical_fire_at_us,id",
            schedule,
        )

    try:
        for path in gates.iterdir():
            path.unlink()
        with coordinator(root, cli) as registration:
            client = Client(registration)
            a, b, blocked = str(uuid4()), str(uuid4()), str(uuid4())
            value = definition("Concurrent slow", "ops/slow")
            saved_a = post(
                client, "/api/v1/schedules", {"id": a, "paused": True, "definition": value}
            )
            saved_b = post(
                client,
                "/api/v1/schedules",
                {
                    "id": b,
                    "paused": True,
                    "definition": definition("Concurrent other", "ops/other"),
                },
            )
            saved_blocked = post(
                client,
                "/api/v1/schedules",
                {
                    "id": blocked,
                    "paused": True,
                    "definition": definition("Reserved output", "ops/slow"),
                },
            )
            post(client, f"/api/v1/schedules/{a}/run", {}, saved_a["etag"])
            wait(lambda: (gates / "slow-started").exists())
            post(client, f"/api/v1/schedules/{b}/run", {}, saved_b["etag"])
            wait(lambda: (gates / "other-started").exists())
            assert state(a)[0][0] == state(b)[0][0] == "RUNNING"
            assert rows("SELECT count(*) FROM write_reservations") == [(2,)], (
                state(a),
                state(b),
                rows("SELECT state,failure_class FROM attempts"),
            )
            post(client, f"/api/v1/schedules/{blocked}/run", {}, saved_blocked["etag"])
            assert state(blocked) == [("QUEUED", None)]
            # Schedule edits affect a new request; the first accepted build keeps its branch/policy.
            value["build"]["data_branch"] = "overlap-independent"
            value["policies"]["allow_overlapping_builds"] = True
            edited = post(client, f"/api/v1/schedules/{a}", value, saved_a["etag"], method="PUT")
            post(client, f"/api/v1/schedules/{a}/run", {}, edited["etag"])
            (gates / "other-release").touch()
            wait(lambda: state(b)[0][0] == "SUCCEEDED")
            wait(lambda: len(state(a)) == 2 and all(row[0] == "RUNNING" for row in state(a)))
            assert rows(
                "SELECT b.name FROM write_reservations r JOIN data_branches b ON b.id=r.branch_id "
                "ORDER BY b.name"
            ) == [("overlap",), ("overlap-independent",)]
            assert state(blocked) == [("QUEUED", None)], (
                "conflicting scope must wait without a producer or failed occurrence"
            )
            (gates / "slow-release").touch()
            wait(lambda: all(row[0] == "SUCCEEDED" for row in state(a)))
            wait(lambda: state(blocked)[0][0] == "SUCCEEDED")
            assert rows("SELECT count(*) FROM write_reservations") == [(0,)]
            assert rows(
                "SELECT count(DISTINCT build_id) FROM schedule_occurrences WHERE schedule_id=?", a
            ) == [(2,)]
            retry_id = str(uuid4())
            retry_definition = definition("Retry genuine worker crash", "ops/retry")
            retry_definition["policies"].update(max_attempts=2, retryable_classes=["worker_crash"])
            retry_saved = post(
                client,
                "/api/v1/schedules",
                {"id": retry_id, "paused": True, "definition": retry_definition},
            )
            post(client, f"/api/v1/schedules/{retry_id}/run", {}, retry_saved["etag"])
            wait(lambda: state(retry_id) and state(retry_id)[0][0] == "SUCCEEDED")
            retry_build = state(retry_id)[0][1]
            attempts = rows(
                "SELECT a.state,a.failure_class FROM attempts a JOIN jobs j ON j.id=a.job_id "
                "WHERE j.build_id=? ORDER BY a.attempt_no",
                retry_build,
            )
            assert len(attempts) == 2, attempts
            assert attempts[0][0] == "FAILED" and attempts[1][0] == "SUCCEEDED", attempts

            # IPC submissions obey the same bounded execution-delegate capacity.
            # Two gated jobs occupy it; a third independent accepted build stays
            # durably queued until one finishes, rather than starting another thread.
            for name in ("slow", "other"):
                (gates / f"{name}-started").unlink()
                (gates / f"{name}-release").unlink()
            command("build", "ops/slow", "--branch", "capacity-a", "--force", "--no-wait")
            wait(lambda: (gates / "slow-started").exists())
            command("build", "ops/other", "--branch", "capacity-b", "--force", "--no-wait")
            wait(lambda: (gates / "other-started").exists())
            third = command("build", "ops/slow", "--branch", "capacity-c", "--force", "--no-wait")[
                "data"
            ]["id"]
            # An authenticated command round trip also proves control remains responsive.
            client.context(branch="overlap")
            assert rows("SELECT state FROM builds WHERE id=?", third) == [("QUEUED",)]
            assert rows(
                "SELECT count(*) FROM attempts a JOIN jobs j ON j.id=a.job_id WHERE j.build_id=?",
                third,
            ) == [(0,)]
            (gates / "slow-release").touch()
            (gates / "other-release").touch()
            wait(lambda: rows("SELECT state FROM builds WHERE id=?", third) == [("SUCCEEDED",)])
            wait(lambda: rows("SELECT count(*) FROM write_reservations") == [(0,)])

    finally:
        for name in ("slow", "other"):
            (gates / f"{name}-release").touch()
    return [
        "persistent observation and actions continue during two disjoint gated builds",
        "allow-overlap retains frozen settings; distinct branches run and conflicting scopes wait",
        "waiting occurrence resumes once reservations release, with no duplicate build linkage",
        "scheduled worker_crash retry uses the frozen source and retains both genuine attempts",
        "IPC submissions retain a bounded durable queue while execution delegates are occupied",
    ]
