"""Create an isolated SQL and G3 checks review workspace without overwriting files."""

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
    "orders.py": """from decimal import Decimal
import polars as pl
from transflow import Check, Output, transform
from transflow import expectations as E


@transform(
    output=Output(
        "raw/orders", checks=Check(E.primary_key("order_id"), "Order identifiers")
    )
)
def orders():
    return pl.DataFrame(
        {
            "order_id": [1, 2, 3],
            "customer_id": [1, 2, 3],
            "region": ["east", "west", "east"],
            "amount": pl.Series(
                [Decimal("10.00"), Decimal("20.00"), Decimal("30.00")],
                dtype=pl.Decimal(18, 2),
            ),
        }
    ).lazy()
""",
    "customers.py": """import polars as pl
from transflow import Check, Output, transform
from transflow import expectations as E


@transform(
    output=Output(
        "raw/customers",
        checks=Check(E.primary_key("customer_id"), "Customer identifiers"),
    )
)
def customers():
    return pl.DataFrame({"customer_id": [1, 2, 3]}).lazy()
""",
    "qualified_orders.py": """from transflow import Check, Input, Output, Parameter, sql_transform
from transflow import expectations as E

qualified_orders = sql_transform(
    "qualified_orders.sql",
    orders=Input(
        "raw/orders",
        checks=Check(
            E.input("current_customers")
            .row_count()
            .gt(E.input("previous_customers").row_count()),
            "Current customer list is larger",
        ),
    ),
    current_customers=Input("raw/customers", branch="master", role="validation"),
    previous_customers=Input("raw/customers", branch="review", role="validation"),
    output=Output(
        "curated/qualified_orders",
        checks=[
            Check(E.primary_key("order_id"), "Order identifiers"),
            Check(
                E.col("customer_id").in_dataset(
                    "current_customers", column="customer_id"
                ),
                "Known customers",
            ),
            Check(E.group_by("customer_id").count().lte(1), "One order per customer"),
            Check(
                E.col("region").matches("east|west", mode="full"), "Recognized regions"
            ),
            Check(E.col("amount").null_count().equals(0), "Amounts are present"),
            Check(
                E.col("amount").gte(
                    E.literal(
                        {
                            "type": "decimal",
                            "value": "0.00",
                            "precision": 18,
                            "scale": 2,
                        }
                    )
                ),
                "Nonnegative amounts",
            ),
        ],
    ),
    params={
        "minimum": Parameter(
            {"type": "decimal", "precision": 18, "scale": 2},
            {"type": "decimal", "value": "15.00", "precision": 18, "scale": 2},
        )
    },
)
""",
    "qualified_orders.sql": """SELECT order_id, customer_id, region, amount
FROM "orders"
WHERE amount >= $minimum
ORDER BY order_id
""",
    "report.py": """from transflow import Check, Input, Output, transform
from transflow import expectations as E


@transform(
    rows=Input(
        "curated/qualified_orders",
        checks=Check(
            E.col("customer_id").in_dataset("customers", column="customer_id"),
            "Consumer customer check",
        ),
    ),
    customers=Input("raw/customers", role="validation"),
    output=Output(
        "curated/report",
        checks=Check(
            E.row_count().equals(E.input("rows").row_count()), "Row count is preserved"
        ),
    ),
)
def report(rows):
    return rows
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
            check=False,
        )
        assert result.returncode == 0, result.stdout + result.stderr
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
    run("branch", "create", "review")
    customer_source = root / "src/customers.py"
    original = customer_source.read_text()
    customer_source.write_text(original.replace("[1, 2, 3]", "[1]"))
    try:
        reviewed = run("build", "raw/customers", "--branch", "review", "--force")["data"]
        assert reviewed["state"] == "SUCCEEDED" and len(reviewed["jobs"]) == 1, reviewed
    finally:
        customer_source.write_text(original)
    first = run("build", "curated/report", "--branch", "master")["data"]
    assert first["state"] == "SUCCEEDED" and len(first["jobs"]) == 4, first
    cached = run("build", "curated/report", "--branch", "master")["data"]
    assert all(job["state"] == "CACHED" and not job["attempts"] for job in cached["jobs"])
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
        targets=[datasets["curated/report"]],
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
            ("Delivery after publication", event, "curated/qualified_orders"),
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
                "curated/report",
            ),
        ):
            current = copy.deepcopy(definition)
            current.update(
                name=name, description="Synthetic SQL and checks review", trigger=trigger
            )
            current["build"]["targets"] = [datasets[target]]
            if target == "curated/qualified_orders":
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
            "name": "SQL and checks review",
            "description": "Synthetic SQL, exact checks and pinned branch aliases",
            "datasets": [
                {
                    "identity": f"dataset:{workspace}:{datasets[path]['dataset_id']}",
                    "position": {"x": x, "y": y},
                }
                for path, x, y in (
                    ("raw/orders", 0, 150),
                    ("curated/qualified_orders", 360, 0),
                    ("raw/customers", 360, 300),
                    ("curated/report", 720, 150),
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
        "datasets": 4,
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
