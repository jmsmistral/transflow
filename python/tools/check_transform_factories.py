"""Real CLI/API factory, exact-pin, SQL evidence and last-good regressions."""

from __future__ import annotations

import argparse
import hashlib
import json
import sqlite3
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
from api_client import Client, coordinator  # noqa: E402
from create_factory_demo import create  # noqa: E402


def verify(root: Path, cli: Path, receipt: dict[str, Any]) -> list[str]:
    cases = [
        "fresh ordinary/meta Python exports and SQL factory build five datasets",
        "unchanged captured factory pipeline reuses all five outputs",
    ]

    def invoke(*arguments: str, code: int = 0) -> dict[str, Any]:
        result = subprocess.run(
            [str(cli), "--workspace", str(root), *arguments, "--json"],
            capture_output=True,
            text=True,
            timeout=240,
            check=False,
        )
        assert result.returncode == code, result.stdout + result.stderr
        value: dict[str, Any] = json.loads(result.stdout)["result"]["data"]
        return value

    datasets = {
        d["path"]: d["id"]
        for d in tomllib.loads((root / ".transflow/catalog.toml").read_text())["datasets"]
    }
    sql_id = datasets["analytics/region_summary"]

    def heads() -> list[tuple[Any, ...]]:
        with sqlite3.connect(root / ".transflow/runtime/catalog.sqlite") as db:
            db.execute("PRAGMA query_only=ON")
            return db.execute(
                "SELECT dataset_id,branch_id,version_id FROM dataset_heads ORDER BY 1,2"
            ).fetchall()

    original = heads()
    source = root / "src/region_summary.py"
    authored = source.read_text()
    first = invoke("build", "show", receipt["builds"][0])
    first_job = next(j for j in first["jobs"] if j["dataset"] == sql_id)
    east_job = next(j for j in first["jobs"] if j["dataset"] == datasets["curated/east"])
    east_checks = east_job["attempts"][-1]["checks"]
    assert len([c for c in east_checks if c["phase"] == "output"]) == 2
    assert any(c["phase"] == "input" and c["alias"] == "orders" for c in east_checks)
    assert all(c["outcome"] == "PASS" for c in east_checks)
    cases.append("dynamic regional transform passes its declared input and output checks")

    regional_path = root / "src/factory_helpers.py"
    regional_source = regional_path.read_text()
    filter_expression = 'orders.filter(pl.col("region") == region)'
    assert regional_source.count(filter_expression) == 1
    try:
        regional_path.write_text(
            regional_source.replace(filter_expression, 'orders.filter(pl.col("region") != region)')
        )
        rejected = invoke("build", "curated/east", "--branch", "master", "--force", code=1)
        assert rejected["state"] == "FAILED" and heads() == original
        checks = rejected["jobs"][0]["attempts"][-1]["checks"]
        assert any(
            c["phase"] == "output"
            and c["name"] == "expected_region"
            and c["outcome"] == "VIOLATION"
            for c in checks
        )
        cases.append("dynamic output check rejects the wrong region and preserves every head")
    finally:
        regional_path.write_text(regional_source)
    recovered = invoke("build", "curated/east", "--branch", "master")
    assert recovered["state"] == "SUCCEEDED" and heads() == original
    assert recovered["jobs"][0]["state"] == "CACHED"
    cases.append("restored regional factory reuses its original checked publication")

    def evidence(client: Client, report: dict[str, Any]) -> dict[str, Any]:
        job = next(j for j in report["jobs"] if j["dataset"] == sql_id)
        response = client.call(
            "GET",
            f"/api/v1/attempts/{job['attempts'][-1]['id']}",
            query={"branch": "master", "plan": report["plan"]["id"]},
        )["data"]
        assert isinstance(response, dict)
        return response

    try:
        with coordinator(root, cli) as registration:
            client = Client(registration)
            for path, region in [("curated/east", "east"), ("curated/west", "west")]:
                definition = client.call(
                    "GET", f"/api/v1/datasets/{datasets[path]}", query={"branch": "master"}
                )["data"]
                preview = definition["producer"]["authoring"]["python_preview"]
                assert "Input('raw/orders'" in preview
                assert f"Output('{path}'" in preview and f"== '{region}'" in preview
                assert "def make_" not in preview
            cases.append("Python preview API resolves actual factory paths and closure values")
            sql = evidence(client, first)["generated_sql"]
            assert "FROM deliveries" in sql["sql"] and "$minimum_amount" in sql["sql"]
            assert hashlib.sha256(sql["sql"].encode()).hexdigest() == sql["sha256"]
            assert sql["inputs"][0]["version"] == first_job["inputs"][0]["version"]
            cases.append("API SQL evidence matches durable hash and exact input version")

            # Required input checks must block query generation, not just publication.
            marker = root / "generator-called.txt"
            gated = authored.replace(
                "Input, Output, Parameter, sql_transform",
                "Input, Output, Parameter, Check, sql_transform\n"
                "from transflow import expectations as E\nfrom pathlib import Path",
            )
            gated = gated.replace(
                "deliveries=Input(source)",
                "deliveries=Input(source, checks=Check(E.row_count().gt(1000), "
                '"Required input size", on_error="FAIL"))',
            )
            gated = gated.replace(
                "        assert deliveries.alias",
                f"        Path({str(marker)!r}).touch()\n        assert deliveries.alias",
            )
            source.write_text(gated)
            failed = invoke(
                "build", "analytics/region_summary", "--branch", "master", "--force", code=1
            )
            assert failed["state"] == "FAILED" and not marker.exists() and heads() == original
            failed = invoke("build", "show", failed["id"])
            assert evidence(client, failed)["generated_sql"] is None
            cases.append("required input failure prevents SQL generation and preserves all heads")

            source.write_text(authored.replace("GROUP BY region", "GROUP BY missing_region"))
            failed = invoke(
                "build", "analytics/region_summary", "--branch", "master", "--force", code=1
            )
            assert heads() == original
            failed = invoke("build", "show", failed["id"])
            assert "missing_region" in evidence(client, failed)["generated_sql"]["sql"]
            cases.append("failed SQL execution retains its own generated SQL and last-good data")

            source.write_text(
                authored.replace(
                    '        return """',
                    '        raise RuntimeError("generation failed")\n        return """',
                )
            )
            failed = invoke(
                "build", "analytics/region_summary", "--branch", "master", "--force", code=1
            )
            assert heads() == original
            failed = invoke("build", "show", failed["id"])
            assert evidence(client, failed)["generated_sql"] is None
            cases.append("generation failure has no substituted SQL evidence and preserves heads")

            source.write_text(authored)
            replay = invoke("build", "replay", receipt["builds"][0], "--branch", "factory-replay")
            assert replay["state"] == "SUCCEEDED" and source.read_text() == authored
            cases.append("captured factory replay succeeds without changing live files")

            # A handle-aware selected build uses fallback pins and destination metadata.
            branch = invoke("build", "curated/west", "--branch", "review", "--force")
            assert branch["state"] == "SUCCEEDED" and len(branch["jobs"]) == 1
            cases.append("metadata-aware selected build reads fallback pins on another branch")
    finally:
        source.write_text(authored)
    return cases


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cli", type=Path)
    parser.add_argument("wheel", type=Path)
    parser.add_argument("wheelhouse", type=Path)
    parser.add_argument("--workspace", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    cli, wheel, wheelhouse = (
        p.resolve(strict=True) for p in (args.cli, args.wheel, args.wheelhouse)
    )
    with tempfile.TemporaryDirectory(prefix="tf-factories-") as temporary:
        root = (
            args.workspace.resolve() if args.workspace else Path(temporary).resolve() / "workspace"
        )
        receipt = (
            json.loads((root / "DEMO.json").read_text())
            if args.workspace
            else create(root, cli, wheel, wheelhouse)
        )
        cases = verify(root, cli, receipt)
    args.output.write_text(
        json.dumps({"format_version": 1, "passed": len(cases), "cases": cases}, indent=2) + "\n"
    )
    print(f"Factory CLI/API qualification passed: {len(cases)} journeys")


if __name__ == "__main__":
    main()
