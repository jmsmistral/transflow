"""Native API build/publication/cancellation and foreign metadata qualification."""

from __future__ import annotations

import argparse
import json
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
            check=True,
            timeout=180,
        )
        value: dict[str, Any] = json.loads(p.stdout)
        return value["result"]  # type: ignore[no-any-return]

    with tempfile.TemporaryDirectory(prefix="tf-api-native-") as temporary:
        base = Path(temporary).resolve()
        root, provider = base / "consumer", base / "provider"
        for workspace in [root, provider]:
            run(workspace, "init", str(workspace))
            (workspace / "requirements.in").write_text(
                "polars==1.44.2\nduckdb==1.5.5\npyarrow==25.0.1\n"
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
        code = (
            "import polars as pl\nfrom transflow import transform, Output\n"
            '@transform(output=Output("raw/items"))\n'
            'def items(): return pl.DataFrame({"id":[1,2]}).lazy()\n'
        )
        (root / "src/items.py").write_text(code)
        (provider / "src/items.py").write_text(code)
        run(provider, "build", "raw/items", "--branch", "main", "--python", sys.executable)
        provider_id = tomllib.loads((provider / "workspace.toml").read_text())["workspace_id"]
        provider_dataset = tomllib.loads((provider / ".transflow/catalog.toml").read_text())[
            "datasets"
        ][0]["id"]

        def rows(sql: str, *params: object) -> list[tuple[Any, ...]]:
            with closing(sqlite3.connect(root / ".transflow/runtime/catalog.sqlite")) as db:
                return db.execute(sql, params).fetchall()

        def wait(client: Client, build: str) -> dict[str, Any]:
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                # Inspect the accepted exact source through its saved plan context.
                plan = rows("SELECT plan_id FROM builds WHERE id=?", build)[0][0]
                response = client.call(
                    "GET",
                    f"/api/v1/builds/{build}",
                    query={"branch": "main", "plan": plan},
                    status=None,
                )
                if "error" in response:
                    assert response["error"]["code"] == "TF_API_CONFLICT", response
                    time.sleep(0.03)
                    continue
                value = response["data"]
                if value["state"] in {"SUCCEEDED", "FAILED", "CANCELED", "INTERRUPTED"}:
                    return value  # type: ignore[no-any-return]
                time.sleep(0.03)
            raise AssertionError("build did not settle")

        with coordinator(root, binary) as registration:
            client = Client(registration)
            selection = {"branch": "main", "targets": ["raw/items"], "python": sys.executable}
            context, key = client.context(), str(uuid4())
            before = rows("SELECT count(*) FROM builds")
            first = client.mutate(
                "/api/v1/builds",
                {"kind": "request", "request": selection},
                context=context,
                key=key,
            )["data"]
            repeated = client.mutate(
                "/api/v1/builds",
                {"kind": "request", "request": selection},
                context=context,
                key=key,
            )["data"]
            assert repeated == first
            report = wait(client, first["build"])
            assert report["state"] == "SUCCEEDED", report
            assert rows("SELECT count(*) FROM builds")[0][0] == before[0][0] + 1
            assert rows("SELECT count(*) FROM dataset_versions") == [(1,)]
            cases.append(
                "new HTTP build survives client disconnect and retry creates one build/publication"
            )
            dataset = report["jobs"][0]["dataset"]
            version = report["jobs"][0]["version"]
            metadata = client.call(
                "GET", f"/api/v1/datasets/{dataset}/versions", query={"branch": "main"}
            )["data"]["entries"][0]
            assert metadata["row_count"] == "2" and metadata["version"] == version
            historical = client.context(dataset=dataset, version=version)
            assert historical["context"]["source"] == report["source"]
            client.call(
                "GET",
                "/api/v1/lineage",
                query={
                    "branch": "main",
                    "dataset": dataset,
                    "version": version,
                    "start": "raw/items",
                },
            )
            cases.append(
                "local version context binds exact source/lineage/schema/row-count metadata"
            )
            draft = client.mutate("/api/v1/plans", selection)["data"]
            accepted = client.mutate(
                "/api/v1/builds",
                {"kind": "plan", "plan_id": draft["plan_id"]},
                plan=draft["plan_id"],
            )["data"]
            assert wait(client, accepted["build"])["state"] == "SUCCEEDED"
            client.mutate(
                "/api/v1/builds",
                {"kind": "plan", "plan_id": draft["plan_id"]},
                plan=draft["plan_id"],
                status=409,
            )
            cases.append("saved-plan acceptance is explicit and cannot be accepted twice")
            client.mutate(
                "/api/v1/externals",
                {
                    "operation": "add",
                    "workspace": str(provider),
                    "dataset": "raw/items",
                    "as": "provider.items",
                    "branch": "main",
                },
            )
            (root / "src/consumer.py").write_text(
                "from transflow import transform, Input, Output\n"
                '@transform(rows=Input("external/provider/items"),output=Output("curated/external"))\n'
                "def consume(rows): return rows\n"
            )
            external = client.mutate(
                "/api/v1/builds",
                {"kind": "request", "request": {**selection, "targets": ["curated/external"]}},
            )["data"]
            foreign = wait(client, external["build"])
            assert foreign["state"] == "SUCCEEDED", foreign
            foreign_version = foreign["jobs"][0]["inputs"][0]["version"]
            # Reading retained foreign metadata must not import changed provider code.
            (provider / "src/items.py").write_text(
                "raise RuntimeError('provider read must not import')\n"
            )
            foreign_context = client.context(
                dataset=provider_dataset, version=foreign_version, origin_workspace=provider_id
            )
            assert foreign_context["context"]["source"] is None
            assert foreign_context["context"]["selection"]["kind"] == "foreign_version"
            versions = client.call(
                "GET",
                f"/api/v1/datasets/{provider_dataset}/versions",
                query={"branch": "main", "origin_workspace": provider_id},
            )
            assert versions["data"]["entries"][0]["availability"] == "not_verified"
            assert rows("SELECT count(*) FROM replicas") == [(0,)]
            cases.append(
                "foreign API registration/build uses provider-owned bytes; "
                "retained reads import no provider code"
            )
            # Synchronize cancellation against an observable producer marker, not a sleep guess.
            marker = base / "running"
            release = base / "release"
            slow = "import time\nfrom pathlib import Path\n" + code.replace(
                "def items(): return",
                (
                    f'def items():\n    Path({str(marker)!r}).write_text("ready")\n'
                    f"    while not Path({str(release)!r}).exists(): time.sleep(0.02)\n    return"
                ),
            )
            (root / "src/items.py").write_text(slow)
            before_heads = rows(
                "SELECT branch_id,dataset_id,version_id FROM dataset_heads "
                "ORDER BY branch_id,dataset_id"
            )
            pending = client.mutate(
                "/api/v1/builds", {"kind": "request", "request": {**selection, "force": True}}
            )["data"]
            deadline = time.monotonic() + 60
            while not marker.exists() and time.monotonic() < deadline:
                time.sleep(0.02)
            assert marker.exists()
            wrong = client.call("GET", "/api/v1/context", query={"branch": "other"})
            client.call(
                "POST",
                f"/api/v1/builds/{pending['build']}/cancel",
                {},
                query={"branch": "other"},
                headers={
                    "If-Match": '"' + wrong["context"]["fingerprint"] + '"',
                    "Idempotency-Key": str(uuid4()),
                },
                status=409,
            )
            key, context = str(uuid4()), client.context()
            canceled = client.mutate(
                f"/api/v1/builds/{pending['build']}/cancel", {}, context=context, key=key
            )
            repeated = client.mutate(
                f"/api/v1/builds/{pending['build']}/cancel", {}, context=context, key=key
            )
            assert canceled["data"] == repeated["data"]
            assert wait(client, pending["build"])["state"] == "CANCELED"
            assert (
                rows(
                    "SELECT branch_id,dataset_id,version_id FROM dataset_heads "
                    "ORDER BY branch_id,dataset_id"
                )
                == before_heads
            )
            cases.append(
                "active HTTP cancellation is replayable and preserves every last-good head"
            )
        with coordinator(root, binary) as registration:
            client = Client(registration)
            assert (
                client.mutate(
                    f"/api/v1/builds/{pending['build']}/cancel", {}, context=context, key=key
                )["data"]
                == canceled["data"]
            )
            cases.append("cancellation receipt survives coordinator restart")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps({"tasks": ["T074", "T075", "T076"], "cases": cases}, indent=2) + "\n"
    )


if __name__ == "__main__":
    main()
