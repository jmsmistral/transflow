"""T102/T103 real Parquet checks, exact ratios and pinned alias semantics."""

from __future__ import annotations

import importlib
import json
import subprocess
import sys
import tempfile
from datetime import UTC
from decimal import Decimal
from pathlib import Path
from typing import Any
from uuid import uuid4

REPO = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(REPO / "python/sdk/src"), str(REPO / "python/worker/src")]

from transflow import Check  # noqa: E402
from transflow import expectations as E  # noqa: E402
from transflow_worker.discovery import _check  # noqa: E402


def main() -> None:
    probe, stage, wheel = (str(Path(v).resolve(strict=True)) for v in sys.argv[1:4])
    pa, pq = importlib.import_module("pyarrow"), importlib.import_module("pyarrow.parquet")
    cases: list[str] = []

    def command(args: list[str]) -> dict[str, Any]:
        p = subprocess.run(args, capture_output=True, text=True, timeout=120, check=False)
        assert p.returncode == 0, p.stdout + p.stderr
        result: dict[str, Any] = json.loads(p.stdout)
        return result

    with tempfile.TemporaryDirectory(prefix="tf-g3-checks-") as temporary:
        root = Path(temporary).resolve()
        launcher = root / "worker"
        launcher.write_text(
            f"#!{sys.executable} -I\nimport sys\nsys.path.insert(0, {wheel!r})\n"
            "from transflow_worker.cli import main\n"
            "raise SystemExit(main(sys.argv[sys.argv.index('transflow_worker')+1:]))\n"
        )
        launcher.chmod(0o700)
        clock = {
            "type": "timestamp",
            "value": "2026-10-13T12:00:00.000000Z",
            "unit": "us",
            "timezone": "UTC",
        }

        def run(
            name: str,
            table: Any,
            checks: list[Check],
            expected: list[str],
            *,
            inputs: dict[str, Any] | None = None,
            at: dict[str, object] | None = None,
        ) -> dict[str, Any]:
            directory = root / name
            directory.mkdir()
            workspace = directory / "workspace"

            def stage_table(label: str, table: Any) -> dict[str, Any]:
                source = directory / label
                source.mkdir()
                pq.write_table(table, source / "part.parquet")
                return command([stage, "stage", str(workspace), str(source), "part.parquet"])

            subject = stage_table("subject", table)
            dataset = {"workspace_id": str(uuid4()), "dataset_id": str(uuid4())}
            bindings = [
                {
                    "alias": alias,
                    "dataset": dataset,
                    "version_id": str(uuid4()),
                    **stage_table(alias, foreign),
                }
                for alias, foreign in (inputs or {}).items()
            ]
            output = directory / "output"
            output.mkdir(mode=0o700)
            request = {
                "format_version": 1,
                "protocol": {"major": 1, "minor": 0},
                "request_id": str(uuid4()),
                "attempt_id": str(uuid4()),
                "auth_token": "0" * 64,
                "result_directory": str(output),
                **subject,
                "phase": "input",
                "subject_version": str(uuid4()),
                "consumer_definition": "a" * 64,
                "binding": "rows",
                "threads": "1",
                "memory_bytes": None,
                "spill_bytes": "268435456",
                "checks": [_check(c.effective()) for c in checks],
                "queries": [],
                "inputs": bindings,
                "evaluation_time": at if at is not None else clock,
            }
            path = directory / "request.json"
            path.write_text(json.dumps(request))
            result = command([probe, str(workspace), str(launcher), str(path)])
            actual = result["result"]["checks"]
            assert [c["status"] for c in actual] == expected, (name, result)
            if result["worker"] is not None:
                assert result["worker"]["logs_complete"] and not result["worker"]["stderr"], result
            if bindings:
                expected_bindings = [
                    {k: v[k] for k in ["alias", "dataset", "version_id", "artifact_digest"]}
                    for v in bindings
                ]
                assert result["result"]["inputs"] == expected_bindings
            cases.append(name)
            return result

        def c(e: E.Expectation, name: str = "check", **kwargs: Any) -> Check:
            return Check(e, name, **kwargs)

        run(
            "finite-approx",
            pa.table({"v": [1.0, 1.01, None, float("nan"), float("inf"), -float("inf")]}),
            [c(E.col("v").approx(1.0, abs_tol=0.02, rel_tol=0.0))],
            ["VIOLATION"],
        )
        run(
            "approx-ignore-null",
            pa.table({"v": [1.0, None]}),
            [c(E.col("v").approx(1.0, abs_tol=0.0, rel_tol=0.0), null_policy="ignore")],
            ["PASS"],
        )
        run(
            "approx-extreme-float",
            pa.table({"v": [1e308]}),
            [
                c(E.col("v").approx(-1e308, abs_tol=0.0, rel_tol=1.5), "below"),
                c(E.col("v").approx(-1e308, abs_tol=0.0, rel_tol=2.0), "boundary"),
            ],
            ["VIOLATION", "PASS"],
        )
        run(
            "approx-exact-uint64",
            pa.table({"v": pa.array([2**64 - 1], type=pa.uint64())}),
            [c(E.col("v").approx(2**64 - 2, abs_tol=0.5, rel_tol=0.0))],
            ["VIOLATION"],
        )
        run(
            "numeric-three-valued-composition",
            pa.table({"v": [1.0, None]}),
            [c(E.any(E.col("v").is_null(), E.col("v").approx(1.0, abs_tol=0.0, rel_tol=0.0)))],
            ["PASS"],
        )
        decimal = E.literal({"type": "decimal", "value": "1.20", "precision": 18, "scale": 2})
        tolerance = E.literal({"type": "decimal", "value": "0.01", "precision": 18, "scale": 2})
        run(
            "decimal-approx",
            pa.table(
                {"v": pa.array([Decimal("1.20"), Decimal("1.21")], type=pa.decimal128(18, 2))}
            ),
            [c(E.col("v").approx(decimal, abs_tol=tolerance, rel_tol=0))],
            ["PASS"],
        )
        run(
            "exact-metrics",
            pa.table({"v": [1, None, 1, 2]}),
            [
                c(E.col("v").null_count().equals(1), "nulls"),
                c(E.col("v").distinct_count().equals(2), "distinct"),
                c(E.col("v").null_fraction().equals(0.25), "fraction"),
            ],
            ["PASS"] * 3,
        )
        run(
            "ratio-no-rounding",
            pa.table({"v": [None, 1, 2]}),
            [
                c(E.col("v").null_fraction().equals(1 / 3)),
                c(E.col("v").null_fraction().gt(1 / 3), "greater"),
            ],
            ["VIOLATION", "PASS"],
        )
        empty = pa.table({"v": pa.array([], type=pa.int64())})
        run(
            "undefined-empty-fraction",
            empty,
            [
                c(E.col("v").null_fraction().equals(0), on_error="WARN"),
                c(
                    E.any(E.row_count().equals(0), E.col("v").null_fraction().equals(0)),
                    "no-shortcut",
                ),
            ],
            ["ERROR", "ERROR"],
        )
        run(
            "group-null-key",
            pa.table({"key": [None, None, "a", "b", "b"]}),
            [
                c(E.group_by("key").count().lte(2)),
                c(E.group_by("key").count().equals(2), "all-groups"),
            ],
            ["PASS", "VIOLATION"],
        )
        run("empty-groups", empty, [c(E.group_by("v").count().gt(0))], ["PASS"])
        run(
            "invalid-regex-on-empty",
            pa.table({"label": pa.array([], type=pa.string())}),
            [c(E.col("label").matches("[z-a]", mode="full"), on_error="WARN")],
            ["ERROR"],
        )
        labels = pa.table({"label": ["abc", "xabcx", None]})
        run(
            "regex-explicit-mode",
            labels,
            [
                c(E.col("label").matches("abc", mode="partial"), "partial", null_policy="ignore"),
                c(E.col("label").matches("abc", mode="full"), "full", null_policy="ignore"),
            ],
            ["PASS", "VIOLATION"],
        )
        run(
            "regex-unsupported-blocks-warn",
            labels,
            [c(E.col("label").matches("(?=abc)", mode="partial"), on_error="WARN")],
            ["ERROR"],
        )
        schema = {
            "format_version": 1,
            "fields": [{"name": "v", "nullable": True, "logical_type": {"type": "i64"}}],
        }
        run(
            "ordered-schema",
            pa.table({"v": [1]}),
            [
                c(E.schema().equals(schema)),
                c(E.schema().equals({"format_version": 1, "fields": []}), "different"),
            ],
            ["PASS", "VIOLATION"],
        )
        from datetime import datetime

        run(
            "frozen-time",
            pa.table(
                {
                    "stamp": pa.array(
                        [datetime(2026, 10, 13, 11, 59, 30, tzinfo=UTC)],
                        type=pa.timestamp("us", tz="UTC"),
                    )
                }
            ),
            [
                c(E.col("stamp").age_seconds_at(E.evaluation_time()).lte(30)),
                c(E.col("stamp").age_seconds_at(E.evaluation_time()).lte(29), "old"),
            ],
            ["PASS", "VIOLATION"],
        )
        run(
            "foreign-membership",
            pa.table({"key": [1, 2, None]}),
            [c(E.col("key").in_dataset("reference", column="id"), null_policy="ignore")],
            ["PASS"],
            inputs={"reference": pa.table({"id": [1, 2, None]})},
        )
        run(
            "foreign-mismatch",
            pa.table({"key": [1, 3]}),
            [c(E.col("key").in_dataset("reference", column="id"))],
            ["VIOLATION"],
            inputs={"reference": pa.table({"id": [1, 2]})},
        )
        run(
            "same-dataset-two-aliases",
            pa.table({"key": [1]}),
            [
                c(E.row_count().equals(E.input("previous").row_count())),
                c(E.row_count().lt(E.input("current").row_count()), "different-version"),
            ],
            ["PASS", "PASS"],
            inputs={"previous": pa.table({"id": [1]}), "current": pa.table({"id": [1, 2]})},
        )
        run(
            "undeclared-input",
            pa.table({"key": [1]}),
            [c(E.col("key").in_dataset("hidden", column="id"), on_error="WARN")],
            ["ERROR"],
        )
        run(
            "missing-foreign-column",
            pa.table({"key": [1]}),
            [c(E.col("key").in_dataset("reference", column="missing"), on_error="WARN")],
            ["ERROR"],
            inputs={"reference": pa.table({"id": [1]})},
        )
    print(
        json.dumps(
            {
                "format_version": 1,
                "cases": cases,
                "count": len(cases),
                "engines": {
                    "duckdb": importlib.import_module("duckdb").__version__,
                    "pyarrow": pa.__version__,
                },
            }
        )
    )


if __name__ == "__main__":
    main()
