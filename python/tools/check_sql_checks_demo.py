"""CLI lifecycle, cache, replay and quality provenance for SQL and G3 checks."""

from __future__ import annotations

import argparse
import json
import sqlite3
import subprocess
import sys
import tempfile
import tomllib
from contextlib import closing
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
from api_client import Client, coordinator  # noqa: E402
from create_sql_checks_demo import create  # noqa: E402


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cli", type=Path)
    parser.add_argument("wheel", type=Path)
    parser.add_argument("wheelhouse", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    cli, wheel, wheelhouse = (
        p.resolve(strict=True) for p in (args.cli, args.wheel, args.wheelhouse)
    )
    cases = []
    with tempfile.TemporaryDirectory(prefix="tf-sql-cli-") as temporary:
        root = Path(temporary).resolve() / "workspace"
        receipt = create(root, cli, wheel, wheelhouse)
        cases.append("four producers, SQL parameters, pinned branch aliases and cached repeat")
        before = (root / "workspace.toml").read_bytes()
        try:
            create(root, cli, wheel, wheelhouse)
        except ValueError:
            pass
        else:
            raise AssertionError("Repeated setup overwrote an existing workspace")
        assert (root / "workspace.toml").read_bytes() == before
        cases.append("existing workspace is preserved")

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

        def query(sql: str) -> list[tuple[Any, ...]]:
            with closing(sqlite3.connect(root / ".transflow/runtime/catalog.sqlite")) as db:
                db.execute("PRAGMA query_only=ON")
                return db.execute(sql).fetchall()

        datasets = {
            d["path"]: d["id"]
            for d in tomllib.loads((root / ".transflow/catalog.toml").read_text())["datasets"]
        }

        def protected_heads() -> list[tuple[Any, ...]]:
            return [
                row
                for row in query(
                    "SELECT dataset_id,branch_id,version_id FROM dataset_heads ORDER BY 1,2"
                )
                if row[0] in {datasets["curated/qualified_orders"], datasets["curated/report"]}
            ]

        original_heads = protected_heads()
        original_sql_version = next(
            row[2] for row in original_heads if row[0] == datasets["curated/qualified_orders"]
        )
        sql = root / "src/qualified_orders.sql"
        original = sql.read_text()
        sql.write_text(original.replace("region, amount", "region, -amount AS amount"))
        failed = invoke("build", "curated/report", "--branch", "master", "--force", code=1)
        assert failed["state"] == "FAILED" and protected_heads() == original_heads, failed
        assert any(j["state"] == "BLOCKED" for j in failed["jobs"])
        cases.append(
            "SQL output violation preserves SQL/downstream heads and blocks descendants under force"
        )
        sql.write_text(original + "\n-- SQL-only change\n")
        changed = invoke("build", "curated/report", "--branch", "master")
        assert changed["state"] == "SUCCEEDED" and any(
            j["state"] == "SUCCEEDED" for j in changed["jobs"]
        ), changed
        cases.append("SQL bytes change the compute identity and execute real jobs")
        repeat = invoke("build", "curated/report", "--branch", "master")
        assert all(j["state"] == "CACHED" and not j["attempts"] for j in repeat["jobs"]), repeat
        cases.append(
            "unchanged SQL/check bindings reuse original publications without new attempts"
        )
        replay = invoke("build", "replay", receipt["builds"][0], "--branch", "replay-review")
        assert replay["state"] == "SUCCEEDED", replay
        assert sql.read_text() == original + "\n-- SQL-only change\n"
        cases.append("replay uses captured SQL and pinned aliases without changing live source")
        results = [
            json.loads(row[0]) for row in query("SELECT envelope_json FROM check_evaluations")
        ]
        contextual = [r for r in results if r.get("inputs")]
        assert contextual and any(
            len({i["version_id"] for i in r["inputs"]}) >= 2 for r in contextual
        )
        cases.append("check evidence retains distinct exact versions for repeated dataset aliases")
        assert query("SELECT count(*) FROM schedules WHERE paused=1")[0][0] == 2
        cases.append("review schedules stay paused")
        with coordinator(root, cli) as registration:
            client = Client(registration)
            for selected in [
                {},
                {
                    "version": original_sql_version,
                    "dataset": datasets["curated/qualified_orders"],
                },
            ]:
                producer = client.call(
                    "GET",
                    f"/api/v1/datasets/{datasets['curated/qualified_orders']}",
                    query={
                        "branch": "master",
                        **selected,
                    },
                )["data"]["producer"]
                assert producer["sql_source"]["path"] == "src/qualified_orders.sql"
                assert len(producer["sql_source"]["sha256"]) == 64
        cases.append("current and historical API definitions expose captured SQL source identity")
    args.output.write_text(
        json.dumps({"format_version": 1, "passed": len(cases), "cases": cases}, indent=2) + "\n"
    )


if __name__ == "__main__":
    main()
