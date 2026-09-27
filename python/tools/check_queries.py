"""Real HTTP query lifecycle, native SQL budgets and provider-owned read qualification."""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import time
import tomllib
from contextlib import closing
from pathlib import Path
from typing import Any
from uuid import uuid4

REPO = Path(__file__).resolve().parents[2]
sys.path[:0] = [
    str(REPO / "python/sdk/src"),
    str(REPO / "python/worker/src"),
    str(REPO / "python/tools"),
]
from api_client import Client, coordinator  # noqa: E402
from transflow_worker.environment import lock_environment, sync_environment  # noqa: E402


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("cli", type=Path)
    parser.add_argument("wheel", type=Path)
    parser.add_argument("wheelhouse", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary, wheel, wheelhouse = (
        p.resolve(strict=True) for p in (args.cli, args.wheel, args.wheelhouse)
    )
    cases: list[str] = []

    def run(root: Path, *args: str) -> dict[str, Any]:
        p = subprocess.run(
            [binary, "--workspace", root, *args, "--json"],
            capture_output=True,
            text=True,
            check=False,
            timeout=180,
        )
        assert p.returncode == 0, (args, p.stdout, p.stderr)
        return json.loads(p.stdout)["result"]  # type: ignore[no-any-return]

    def rows(root: Path, sql: str) -> list[tuple[Any, ...]]:
        with closing(sqlite3.connect(root / ".transflow/runtime/catalog.sqlite")) as db:
            return db.execute(sql).fetchall()

    def settled(client: Client, job: dict[str, Any], expected: str) -> dict[str, Any]:
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            state = client.call(
                "GET", f"/api/v1/queries/{job['data']['id']}", query={"branch": "main"}
            )["data"]
            if state["state"] not in {"QUEUED", "RUNNING"}:
                assert state["state"] == expected, state
                return state  # type: ignore[no-any-return]
            time.sleep(0.03)
        raise AssertionError("Query did not settle")

    with tempfile.TemporaryDirectory(prefix="tf-query-native-") as temporary:
        root, provider = (Path(temporary).resolve() / name for name in ("consumer", "provider"))
        for workspace in (root, provider):
            run(workspace, "init", str(workspace))
            (workspace / "requirements.in").write_text(
                "polars==1.44.2\nduckdb==1.5.5\npyarrow==25.0.1\n"
            )
            with (workspace / "workspace.toml").open("a") as f:
                f.write(
                    "\n[execution]\nmax_jobs=1\ncpu_tokens=1\n"
                    "\n[interactive]\nquery_timeout_seconds=30\n"
                )
            lock_environment(
                workspace,
                "requirements.in",
                "requirements.lock",
                "3.14",
                wheelhouse=wheelhouse,
                offline=True,
            )
            sync_environment(
                workspace,
                "requirements.in",
                "requirements.lock",
                "3.14",
                wheel,
                "0.0.0.dev0",
                wheelhouse=wheelhouse,
                offline=True,
            )
            (workspace / "src/items.py").write_text(
                "import polars as pl\nfrom transflow import transform, Output\n"
                '@transform(output=Output("raw/items"))\n'
                'def items(): return pl.DataFrame({"id":range(10000)}).lazy()\n'
            )
            run(workspace, "build", "raw/items", "--branch", "main", "--python", sys.executable)
        wid = tomllib.loads((root / "workspace.toml").read_text())["workspace_id"]
        foreign = tomllib.loads((provider / "workspace.toml").read_text())["workspace_id"]
        dataset = tomllib.loads((root / ".transflow/catalog.toml").read_text())["datasets"][0]["id"]
        remote = tomllib.loads((provider / ".transflow/catalog.toml").read_text())["datasets"][0][
            "id"
        ]
        version = rows(root, "SELECT id FROM dataset_versions")[0][0]
        remote_version = rows(provider, "SELECT id FROM dataset_versions")[0][0]
        run(
            root,
            "external",
            "add",
            "--workspace",
            str(provider),
            "--dataset",
            "raw/items",
            "--as",
            "market.items",
        )
        objects = sorted(
            p.relative_to(root).as_posix()
            for p in (root / ".transflow/runtime/objects").rglob("*.parquet")
        )
        heads = rows(root, "SELECT * FROM dataset_heads")
        body: dict[str, Any] = {
            "sql": "SELECT id FROM dataset ORDER BY id",
            "parameters": [],
            "bindings": [
                {
                    "alias": "dataset",
                    "dataset": dataset,
                    "origin_workspace": wid,
                    "version": version,
                }
            ],
        }
        with coordinator(root, binary) as registration:
            client = Client(registration)
            assert "query" in client.call("GET", "/api/v1/capabilities")["data"]["operations"]
            context, key = client.context(), str(uuid4())
            job = client.mutate("/api/v1/queries", body, context=context, key=key)
            repeat = client.mutate("/api/v1/queries", body, context=context, key=key)
            assert repeat["data"]["id"] == job["data"]["id"]
            client.mutate(
                "/api/v1/queries", {**body, "sql": "SELECT 1"}, context=context, key=key, status=409
            )
            state = settled(client, job, "SUCCEEDED")
            assert state["rows"] == "1000" and state["truncated"]
            page = client.call(
                "GET",
                f"/api/v1/queries/{state['id']}/results",
                query={"branch": "main", "limit": "2"},
            )["data"]
            assert page["rows"] == [
                [{"type": "i64", "value": "0"}],
                [{"type": "i64", "value": "1"}],
            ], page
            assert page["next_offset"] == 2
            client.call(
                "GET", f"/api/v1/queries/{state['id']}", query={"branch": "other"}, status=409
            )
            cases.append("exact typed pagination, row ceiling, context and idempotent creation")
            query = {
                **body,
                "sql": "SELECT sum(id) AS total FROM other",
                "spill_bytes": "0",
                "bindings": [
                    {
                        "alias": "other",
                        "dataset": remote,
                        "origin_workspace": foreign,
                        "version": remote_version,
                    }
                ],
            }
            job = client.mutate("/api/v1/queries", query)
            state = settled(client, job, "SUCCEEDED")
            assert state["rows"] == "1" and not state["truncated"]
            cases.append("foreign exact version read directly with zero spill allowance")
            for sql in ("DELETE FROM dataset", "SELECT * FROM read_parquet('/tmp/forbidden')"):
                settled(client, client.mutate("/api/v1/queries", {**body, "sql": sql}), "FAILED")
            cases.append("forbidden SQL fails closed and releases protected reads")
            big = {
                **body,
                "sql": "SELECT ? AS text FROM dataset",
                "parameters": ["x" * 4096],
                "bytes": 4096,
            }
            result = settled(client, client.mutate("/api/v1/queries", big), "SUCCEEDED")
            assert result["truncated"] and result["rows"] == "0"
            cases.append("byte limit returns bounded truncated results")
            expensive = {
                **body,
                "sql": "SELECT sum(a.id*b.id+c.id) AS total FROM dataset a "
                "CROSS JOIN dataset b CROSS JOIN dataset c LIMIT 1",
            }
            # Ordinary success cases use the product default; only the expensive-query
            # case lowers the deadline. Busy CI startup must not make successes flaky.
            settings = root / "workspace.toml"
            settings.write_text(
                settings.read_text().replace("query_timeout_seconds=30", "query_timeout_seconds=8")
            )
            result = settled(client, client.mutate("/api/v1/queries", expensive), "TIMED_OUT")
            assert "interactive.query_timeout_seconds" in result["error"]
            cases.append("LIMIT 1 does not evade interactive execution deadline")
        # Explicit zero disables only the query deadline. Cancellation must still stop it.
        config = (
            (root / "workspace.toml")
            .read_text()
            .replace("query_timeout_seconds=8", "query_timeout_seconds=0")
        )
        (root / "workspace.toml").write_text(config)
        with coordinator(root, binary) as registration:
            client = Client(registration)
            job = client.mutate("/api/v1/queries", expensive)
            helper = root / ".transflow/runtime/queries" / job["data"]["id"] / "helper"
            deadline = time.monotonic() + 15
            while not (helper / "recovery.json").exists() and time.monotonic() < deadline:
                time.sleep(0.02)
            assert (helper / "recovery.json").exists(), list(helper.glob("*"))
            pid = json.loads((helper / "recovery.json").read_text())["pid"]
            queued = client.mutate("/api/v1/queries", body)
            assert queued["data"]["state"] == "QUEUED"
            client.mutate(f"/api/v1/queries/{queued['data']['id']}/cancel", {}, context=queued)
            settled(client, queued, "CANCELED")
            client.mutate(f"/api/v1/queries/{job['data']['id']}/cancel", {}, context=job)
            result = settled(client, job, "CANCELED")
            assert result["timeout_seconds"] == "0"
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                pass
            else:
                raise AssertionError("Canceled helper has not been reaped")
            settled(client, client.mutate("/api/v1/queries", {**body, "rows": 2}), "SUCCEEDED")
            cases.append(
                "zero timer, running and queued cancellation, capacity reusable after cleanup"
            )
            stopping = client.mutate("/api/v1/queries", expensive)
            stopping_helper = (
                root / ".transflow/runtime/queries" / stopping["data"]["id"] / "helper"
            )
            deadline = time.monotonic() + 15
            while not (stopping_helper / "recovery.json").exists() and time.monotonic() < deadline:
                time.sleep(0.02)
            assert (stopping_helper / "recovery.json").exists()
        with coordinator(root, binary) as registration:
            Client(registration).call(
                "GET",
                f"/api/v1/queries/{stopping['data']['id']}",
                query={"branch": "main"},
                status=404,
            )
        cases.append("coordinator shutdown cancels live helpers; query IDs expire across restart")
        assert not list((root / ".transflow/runtime/queries").iterdir())
        for workspace in (root, provider):
            assert rows(
                workspace, "SELECT count(*) FROM read_leases WHERE kind='query' AND released=0"
            ) == [(0,)]
        assert rows(root, "SELECT * FROM dataset_heads") == heads
        assert (
            sorted(
                p.relative_to(root).as_posix()
                for p in (root / ".transflow/runtime/objects").rglob("*.parquet")
            )
            == objects
        )
        cases.append(
            "all paths release leases and temporary files without publishing or copying inputs"
        )
    args.output.write_text(json.dumps({"schema": 1, "cases": cases}, indent=2) + "\n")
    print(f"Query qualification passed: {len(cases)} groups")


if __name__ == "__main__":
    main()
