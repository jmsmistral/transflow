"""Create a synthetic operational-alpha review workspace without overwriting files."""

from __future__ import annotations

import argparse
import copy
import json
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any
from uuid import uuid4

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "python/tools"))
from api_client import Client, coordinator  # noqa: E402

SOURCES = {
    "orders.py": """import time
import polars as pl
from transflow import Check, Output, transform
from transflow import expectations as E


@transform(
    output=Output(
        "raw/orders",
        checks=[
            Check(E.col("amount").gte(0), "Amounts are nonnegative", id="amount", on_error="FAIL"),
        ],
    ),
)
def orders():
    time.sleep(1)
    ids = list(range(1, 121))
    return pl.DataFrame(
        {
            "id": ids,
            "region": ["east" if n % 2 else "west" for n in ids],
            "amount": [150 + (n * 17) % 850 for n in ids],
        }
    ).lazy()
""",
    "east.py": """import time
import polars as pl
from transflow import Input, Output, transform


@transform(
    output=Output("curated/east"),
    rows=Input("raw/orders"),
)
def east(rows):
    time.sleep(2)
    return rows.filter(pl.col("region") == "east")
""",
    "west.py": """import time
import polars as pl
from transflow import Input, Output, transform


@transform(
    output=Output("curated/west"),
    rows=Input("raw/orders"),
)
def west(rows):
    time.sleep(2)
    return rows.filter(pl.col("region") == "west")
""",
    "delivery.py": """import time
import polars as pl
from transflow import Input, Output, transform


@transform(
    output=Output("curated/delivery"),
    east=Input("curated/east"),
    west=Input("curated/west"),
)
def delivery(east, west):
    time.sleep(1)
    return pl.concat([east, west])
""",
    "region_summary.py": """from transflow import Input, Output, sql_transform

region_summary = sql_transform(
    output=Output("analytics/region_summary"),
    deliveries=Input("curated/delivery"),
    sql_file="region_summary.sql",
)
""",
    "region_summary.sql": """-- The build pins the exact curated/delivery version behind this alias.
SELECT
    region,
    COUNT(*) AS order_count,
    CAST(SUM(amount) AS BIGINT) AS total_amount,
    ROUND(AVG(amount), 2) AS average_amount
FROM "deliveries"
GROUP BY region
ORDER BY region
""",
}


def create(root: Path, cli: Path, wheel: Path, wheelhouse: Path) -> dict[str, Any]:
    """Use the public CLI and authenticated API to seed real history and definitions."""
    if root.exists():
        raise ValueError("Refusing to overwrite an existing review workspace")

    def run(*args: str) -> dict[str, Any]:
        result = subprocess.run(
            [str(cli), "--workspace", str(root), *args, "--json"],
            text=True,
            capture_output=True,
            timeout=180,
            check=True,
        )
        return json.loads(result.stdout)["result"]  # type: ignore[no-any-return]

    run("init", str(root))
    (root / "requirements.in").write_text("polars==1.44.2\nduckdb==1.5.5\npyarrow==25.0.1\n")
    environment = ["--python", sys.executable, "--no-index", "--wheelhouse", str(wheelhouse)]
    run("env", "lock", *environment)
    run("env", "sync", *environment, "--runtime-wheel", str(wheel))
    (root / "transflow.local.toml").write_text(
        "[python]\nexecutable=" + json.dumps(sys.executable) + "\n"
    )
    with (root / "workspace.toml").open("a") as config:
        config.write("\n[execution]\nmax_jobs=2\ncpu_tokens=2\n")
    for name, source in SOURCES.items():
        (root / "src" / name).write_text(source)
    first = run("build", "analytics/region_summary", "--mode", "full", "--branch", "master")["data"]
    assert first["state"] == "SUCCEEDED" and len(first["jobs"]) == 5, first
    cached = run("build", "analytics/region_summary", "--mode", "full", "--branch", "master")[
        "data"
    ]
    assert all(job["state"] == "CACHED" and not job["attempts"] for job in cached["jobs"])
    run("branch", "create", "review")
    # The lineage picker lists branches with data for visible nodes. Give review
    # one real head while leaving regional/delivery data on master for fallback.
    reviewed = run("build", "raw/orders", "--branch", "review", "--force")["data"]
    assert reviewed["state"] == "SUCCEEDED" and len(reviewed["jobs"]) == 1, reviewed
    workspace = tomllib.loads((root / "workspace.toml").read_text())["workspace_id"]
    datasets = {
        entry["path"]: {"workspace_id": workspace, "dataset_id": entry["id"]}
        for entry in tomllib.loads((root / ".transflow/catalog.toml").read_text())["datasets"]
    }
    fixtures = json.loads((REPO / "schemas/fixtures/conformance.json").read_text())
    definition = copy.deepcopy(
        next(case["value"] for case in fixtures if case["name"] == "schedule-definition-fixed")
    )
    definition["build"].update(
        # The first implicit build captured the registry before assigning new IDs.
        # The cached repeat retains a validated snapshot containing those identities.
        source={"kind": "fixed_snapshot", "snapshot_id": cached["source"]},
        data_branch="master",
        fallback_branches=[],
        input_fallback_policy={"default": [], "rules": {}},
        provider_fallback_policies={},
        targets=[datasets["curated/delivery"]],
        build_mode="full",
        boundaries=[],
        exclusions=[],
        refresh_sources=[],
        parameters={},
        force=True,
    )
    definition["policies"].update(max_attempts=2, retryable_classes=["worker_crash"])
    event = {
        "kind": "dataset_published",
        "id": "orders_ready",
        "dataset": datasets["raw/orders"],
        "branch": "master",
        "payload_mode": "pin",
        "include_resets": False,
    }
    schedules = []
    with coordinator(root, cli) as registration:
        client = Client(registration)
        for name, trigger, target in (
            ("Delivery after publication", event, "curated/east"),
            (
                "Weekday delivery",
                {
                    "kind": "and",
                    "children": [
                        {
                            "kind": "cron",
                            "id": "weekday",
                            "expression": "0 9 * * 1-5",
                            "timezone": "Asia/Dubai",
                            "duplicate_time": "earliest",
                        },
                        event,
                    ],
                },
                "curated/delivery",
            ),
        ):
            current = copy.deepcopy(definition)
            current.update(
                name=name, description="Synthetic operational-alpha review", trigger=trigger
            )
            current["build"]["targets"] = [datasets[target]]
            if target == "curated/east":
                current["build"]["build_mode"] = "selected"
            schedule_id = str(uuid4())
            client.call(
                "POST",
                "/api/v1/schedules",
                {"id": schedule_id, "paused": True, "definition": current},
                headers={"Idempotency-Key": str(uuid4())},
            )
            schedules.append(schedule_id)
        context = client.call("GET", "/api/v1/context", query={"branch": "master"})
        view_id = str(uuid4())
        view = {
            "format_version": 1,
            "id": view_id,
            "revision": 0,
            "name": "Operational alpha review",
            "description": "120 synthetic orders, parallel regions, delivery and a SQL summary",
            "datasets": [
                {
                    "identity": f"dataset:{workspace}:{datasets[path]['dataset_id']}",
                    "position": {"x": x, "y": y},
                }
                for path, x, y in (
                    ("raw/orders", 0, 150),
                    ("curated/east", 360, 0),
                    ("curated/west", 360, 300),
                    ("curated/delivery", 720, 150),
                    ("analytics/region_summary", 1080, 150),
                )
            ],
            "colour": "status",
            "selector": {"branch": "master", "fallback": []},
        }
        client.call(
            "POST",
            "/api/v1/views",
            view,
            query={"branch": "master"},
            headers={
                "If-Match": '"' + context["context"]["fingerprint"] + '"',
                "Idempotency-Key": str(uuid4()),
            },
        )
    receipt = {
        "workspace": str(root),
        "datasets": 5,
        "builds": [first["id"], cached["id"], reviewed["id"]],
        "schedules": schedules,
        "view": view_id,
        "coordinator": "stopped",
    }
    (root / "DEMO.json").write_text(json.dumps(receipt, indent=2) + "\n")
    return receipt


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("workspace", type=Path)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--wheel", type=Path, required=True)
    parser.add_argument("--wheelhouse", type=Path, required=True)
    args = parser.parse_args()
    receipt = create(
        args.workspace.expanduser().resolve(),
        args.cli.resolve(strict=True),
        args.wheel.resolve(strict=True),
        args.wheelhouse.resolve(strict=True),
    )
    print(json.dumps(receipt))


if __name__ == "__main__":
    main()
