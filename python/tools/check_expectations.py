"""Native T061 conformance: real Rust compilation/supervision over exact Parquet subjects."""

from __future__ import annotations

import importlib
import json
import subprocess
import sys
import tempfile
from decimal import Decimal
from pathlib import Path
from typing import Any
from uuid import uuid4

REPO = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(REPO / "python/sdk/src"), str(REPO / "python/worker/src")]
from transflow import Check  # noqa: E402
from transflow import expectations as E  # noqa: E402
from transflow_worker.discovery import _check  # noqa: E402


def command(args: list[str]) -> dict[str, Any]:
    p = subprocess.run(args, capture_output=True, text=True, timeout=180, check=False)
    assert p.returncode == 0, p.stdout + p.stderr
    result: dict[str, Any] = json.loads(p.stdout)
    return result


def main() -> None:
    probe, stage, wheel = (str(Path(v).resolve(strict=True)) for v in sys.argv[1:4])
    pa = importlib.import_module("pyarrow")
    pq = importlib.import_module("pyarrow.parquet")
    pl = importlib.import_module("polars")
    duckdb = importlib.import_module("duckdb")
    pd = importlib.import_module("pandas")
    cases: list[str] = []
    with tempfile.TemporaryDirectory(prefix="tf-checks-") as temporary:
        root = Path(temporary).resolve()
        launcher = root / "worker"
        launcher.write_text(
            f"#!{sys.executable} -I\nimport sys\nsys.path.insert(0, {wheel!r})\n"
            "from transflow_worker.cli import main\n"
            "raise SystemExit(main(sys.argv[sys.argv.index('transflow_worker')+1:]))\n"
        )
        launcher.chmod(0o700)
        measured = root / "measured-worker"
        measured.write_text(
            f"#!{sys.executable} -I\nimport sys\nsys.path.insert(0, {wheel!r})\n"
            "import json, threading, time\nfrom pathlib import Path\n"
            "from transflow_worker import check_adapter\n"
            "original = check_adapter.connect\n"
            "def connect(request, paths):\n"
            "    db = original(request, paths)\n"
            "    spill = Path(request['spill_directory'])\n"
            "    marker = Path(request['result_directory']) / 'measurement.json'\n"
            "    record = {'spill_directory': str(spill), 'peak_bytes': 0, "
            "'query_started': False}\n"
            "    def observe():\n"
            "        try:\n"
            "            size = sum(p.stat().st_size for p in spill.rglob('*') if p.is_file())\n"
            "            record['peak_bytes'] = max(record['peak_bytes'], size)\n"
            "        except FileNotFoundError: pass\n"
            "    class Measured:\n"
            "        def __enter__(self): return self\n"
            "        def __exit__(self, *args): db.close()\n"
            "        def execute(self, sql, parameters):\n"
            "            record['query_started'] = True\n"
            "            marker.write_text(json.dumps(record))\n"
            "            stop = threading.Event()\n"
            "            def sample():\n"
            "                while not stop.wait(0.001): observe()\n"
            "            thread = threading.Thread(target=sample, daemon=True)\n"
            "            thread.start()\n"
            "            try:\n"
            # Inject a known expensive query only in this synthetic deadline launcher.
            "                if marker.parent.parent.name == 'timeout':\n"
            "                    db.execute('SELECT sum(a.range*b.range) "
            "FROM range(100000) a, range(100000) b')\n"
            "                return db.execute(sql, parameters)\n"
            "            finally:\n"
            "                observe(); stop.set(); thread.join()\n"
            "                marker.write_text(json.dumps(record))\n"
            "    return Measured()\n"
            "check_adapter.connect = connect\n"
            "from transflow_worker.cli import main\n"
            "raise SystemExit(main(sys.argv[sys.argv.index('transflow_worker')+1:]))\n"
        )
        measured.chmod(0o700)

        def run(
            name: str,
            table: Any,
            checks: list[Check],
            *,
            engine: str = "arrow",
            mode: str = "input",
            memory: str | None = None,
            spill: str = "268435456",
            expected: list[str] | None = None,
        ) -> dict[str, Any]:
            directory = root / name
            directory.mkdir()
            source = directory / "source"
            source.mkdir()
            path = source / "part.parquet"
            if engine == "polars":
                pl.from_arrow(table).write_parquet(path)
            elif engine == "pandas":
                df = table.to_pandas(types_mapper=pd.ArrowDtype)
                pq.write_table(
                    pa.Table.from_pandas(df, preserve_index=False).replace_schema_metadata(None),
                    path,
                )
            elif engine == "sql":
                with duckdb.connect() as db:
                    db.from_arrow(table).write_parquet(str(path))
            else:
                pq.write_table(table, path)
            # Extra file must not participate in the scan.
            pq.write_table(pa.table({"extra": [999]}), source / "unlisted.parquet")
            workspace = directory / "workspace"
            subject = command([stage, "stage", str(workspace), str(source), "part.parquet"])
            output = directory / "output"
            output.mkdir(mode=0o700)
            request = {
                "format_version": 1,
                "protocol": {"major": 1, "minor": 0},
                "request_id": str(uuid4()),
                "attempt_id": str(uuid4()),
                "auth_token": "0" * 64,
                "result_directory": str(output),
                "artifact_root": subject["artifact_root"],
                "artifact_digest": subject["artifact_digest"],
                "manifest": subject["manifest"],
                "phase": "input",
                "subject_version": str(uuid4()),
                "consumer_definition": "a" * 64,
                "binding": "rows",
                "threads": "1",
                "memory_bytes": memory,
                "spill_bytes": spill,
                "checks": [_check(c.effective()) for c in checks],
                # Caller SQL is discarded and replaced by the typed compiler.
                "queries": [{"sql": "DROP VIEW subject", "parameters": [], "width": 1}],
            }
            request_path = directory / "request.json"
            request_path.write_text(json.dumps(request))
            result = command(
                [
                    probe,
                    str(workspace),
                    str(measured if name in {"timeout", "spill", "disk-error"} else launcher),
                    str(request_path),
                    mode,
                ]
            )
            actual = result["result"]["checks"]
            if expected is not None:
                assert [c["status"] for c in actual] == expected, (name, result)
            assert result["result"]["artifact_digest"] == subject["artifact_digest"]
            assert all(c["sample"] is None for c in actual)
            if result["worker"] is not None:
                assert result["worker"]["stdout"] == "" and result["worker"]["stderr"] == "", result
                assert result["worker"]["logs_complete"], result
            assert not list((workspace / ".transflow/runtime/objects").glob(".staging-*"))
            if name in {"timeout", "spill", "disk-error"}:
                measurement = json.loads((output / "measurement.json").read_text())
                assert measurement["query_started"]
                assert not Path(measurement["spill_directory"]).exists(), measurement
                result["measurement"] = measurement
            cases.append(name)
            return result

        # Production connection settings deny files outside the approved manifest and
        # configuration changes. This is not a general raw-SQL security boundary.
        from transflow_worker.check_adapter import connect

        approved = root / "approved.parquet"
        denied = root / "denied.parquet"
        pq.write_table(pa.table({"x": [1]}), approved)
        pq.write_table(pa.table({"x": [2]}), denied)
        private_spill = root / "security-spill"
        private_spill.mkdir(mode=0o700)
        policy = {
            "spill_directory": str(private_spill),
            "threads": "1",
            "spill_bytes": "1000000",
            "memory_bytes": None,
        }
        with connect(policy, [str(approved)]) as db:
            settings = db.execute(
                "SELECT current_setting('allowed_paths'), "
                "current_setting('allowed_directories'), "
                "current_setting('enable_external_access'), "
                "current_setting('python_enable_replacements')"
            ).fetchone()
            assert settings == ([str(approved)], [str(private_spill) + "/"], False, False), settings
            for sql, params in [
                ("SELECT * FROM read_parquet(?)", [str(denied)]),
                ("SET threads = 2", []),
            ]:
                try:
                    db.execute(sql, params)
                except duckdb.Error:
                    pass
                else:
                    raise AssertionError("Restricted evaluator accepted unapproved access")
        cases.append("restricted-connection")

        def counts(result: dict[str, Any], index: int) -> dict[str, str]:
            return next(
                m["counts"]
                for m in result["result"]["checks"][index]["metrics"]
                if m["path"] == "$"
            )

        table = pa.table(
            {
                "id": pa.array([1, 2, 2, None], pa.int64()),
                "age": pa.array([0, 199, 200, None], pa.int64()),
                "x": pa.array([float("nan"), float("inf"), -0.0, None], pa.float64()),
                "status": ["active", "paused", "active", None],
            }
        )
        age = E.all(E.col("age").non_null(), E.col("age").gte(0), E.col("age").lt(200))
        checks = [
            Check(E.primary_key("id"), "pk"),
            Check(age, "age"),
            Check(age, "ignore age", null_policy="ignore"),
            Check(E.col("age").gte(0), "ignore unknown", null_policy="ignore"),
            Check(E.row_count().equals(4), "count"),
            Check(E.col("status").is_in("active", "paused", None), "membership"),
            Check(E.col("x").is_finite(), "finite"),
            Check(E.col("x").is_nan(), "nan"),
            Check(E.col("missing").exists(), "exists"),
            Check(E.col("age").has_type("string"), "type"),
            Check(E.col("missing").non_null(), "missing", on_error="WARN"),
            Check(E.col("age").is_finite(), "incompatible", on_error="WARN"),
            Check(
                E.any(E.col("age").exists(), E.every(E.col("missing").non_null())),
                "no short circuit",
                on_error="WARN",
            ),
        ]
        expected = [
            "VIOLATION",
            "VIOLATION",
            "VIOLATION",
            "PASS",
            "PASS",
            "PASS",
            "VIOLATION",
            "VIOLATION",
            "VIOLATION",
            "VIOLATION",
            "ERROR",
            "ERROR",
            "ERROR",
        ]
        baseline = None
        for engine in ("arrow", "polars", "pandas", "sql"):
            r = run(engine, table, checks, engine=engine, expected=expected)
            reduced = [
                {k: c[k] for k in ("status", "exact", "failed_rows", "metrics", "error")}
                for c in r["result"]["checks"]
            ]
            if baseline is None:
                baseline = reduced
            else:
                assert reduced == baseline, engine
            assert counts(r, 0) == {
                "null_key_rows": "1",
                "duplicate_groups": "1",
                "duplicate_group_rows": "2",
                "violating_rows": "3",
            }
            assert counts(r, 1)["violating_rows"] == "2"
            assert counts(r, 2)["violating_rows"] == "2"
            assert counts(r, 3)["skipped_rows"] == "1"
        run("candidate", table, checks, mode="output", expected=expected)
        floats = run(
            "float-comparisons",
            table,
            [
                Check(E.col("x").not_equals(float("nan")), "not equal nan"),
                Check(E.col("x").equals(float("nan")), "equal nan"),
                Check(E.col("x").equals(0.0), "signed zero"),
                Check(E.col("x").is_in(None, -0.0), "null membership"),
                Check(E.not_(E.col("x").gt(0.0)), "negated nan"),
            ],
            expected=["VIOLATION"] * 5,
        )
        assert [counts(floats, i)["violating_rows"] for i in range(5)] == ["1", "4", "3", "2", "2"]
        truth = json.loads((REPO / "schemas/fixtures/expectation-semantics-v1.json").read_text())[
            "truth"
        ]
        convert = {"true": True, "false": False, "unknown": None}
        booleans = pa.table(
            {
                "a": pa.array([convert[t["left"]] for t in truth], pa.bool_()),
                "b": pa.array([convert[t["right"]] for t in truth], pa.bool_()),
            }
        )
        a, b = E.col("a").equals(True), E.col("b").equals(True)
        truth_result = run(
            "truth-tables",
            booleans,
            [
                Check(expr, f"{op} {policy}", null_policy=policy)
                for op, expr in [("all", E.all(a, b)), ("any", E.any(a, b))]
                for policy in ("fail", "ignore")
            ],
            expected=["VIOLATION"] * 4,
        )
        assert [counts(truth_result, i)["violating_rows"] for i in range(4)] == ["8", "5", "4", "1"]

        empty = table.slice(0, 0)
        run(
            "empty",
            empty,
            [
                Check(age, "age"),
                Check(E.primary_key("id"), "pk"),
                Check(E.row_count().gt(0), "nonempty"),
            ],
            expected=["PASS", "PASS", "VIOLATION"],
        )
        name = 'odd"; DROP VIEW subject; --'
        run(
            "identifier",
            pa.table({name: ["a' OR TRUE --", "safe"]}),
            [Check(E.col(name).is_in("safe"), "bound")],
            expected=["VIOLATION"],
        )
        tuples = run(
            "tuple",
            pa.table({"a": ["a|b", "a", "a|b", None, None], "b": ["c", "b|c", "c", "x", None]}),
            [Check(E.primary_key("a", "b"), "pk")],
            expected=["VIOLATION"],
        )
        assert counts(tuples, 0) == {
            "null_key_rows": "2",
            "duplicate_groups": "1",
            "duplicate_group_rows": "2",
            "violating_rows": "4",
        }
        precision = pa.table(
            {
                "u": pa.array([2**64 - 1, 2**64 - 2], pa.uint64()),
                "d": pa.array(
                    [
                        Decimal("123456789012345678901234567890123456.77"),
                        Decimal("123456789012345678901234567890123456.78"),
                    ],
                    pa.decimal128(38, 2),
                ),
                "t": pa.array([1, 2], pa.timestamp("ns")),
            }
        )
        r = run(
            "precision",
            precision,
            [
                Check(E.primary_key("u", "d", "t"), "pk"),
                Check(E.col("u").gt(2**63 - 1), "uint"),
                Check(
                    E.col("d").lt(
                        E.literal(
                            {
                                "type": "decimal",
                                "precision": 38,
                                "scale": 2,
                                "value": "123456789012345678901234567890123456.78",
                            }
                        )
                    ),
                    "decimal",
                ),
                Check(
                    E.col("t").lt(
                        E.literal(
                            {
                                "type": "timestamp",
                                "unit": "ns",
                                "timezone": None,
                                "value": "1970-01-01T00:00:00.000000002",
                            }
                        )
                    ),
                    "ns",
                ),
            ],
            expected=["PASS", "PASS", "VIOLATION", "VIOLATION"],
        )
        assert counts(r, 2)["violating_rows"] == "1" and counts(r, 3)["violating_rows"] == "1"
        run(
            "timezone-ns",
            pa.table({"t": pa.array([1, 2], pa.timestamp("ns", "UTC"))}),
            [Check(E.primary_key("t"), "precision", on_error="WARN")],
            expected=["ERROR"],
        )
        run(
            "case-collision",
            pa.table({"A": [1], "a": [2]}),
            [Check(E.col("A").equals(1), "case")],
            expected=["ERROR"],
        )
        run(
            "unsupported-key",
            table,
            [Check(E.primary_key("x"), "pk", on_error="WARN")],
            expected=["ERROR"],
        )
        run(
            "canceled",
            table,
            [Check(age, "age", on_error="WARN")],
            mode="canceled",
            expected=["ERROR"],
        )
        run(
            "disabled",
            table,
            [Check(E.row_count().equals(4), "count")],
            mode="disabled",
            expected=["PASS"],
        )
        # Strict engine budget failure must never be a partial/sampled PASS.
        run(
            "memory-error",
            table,
            [Check(age, "age", on_error="WARN")],
            memory="1",
            expected=["ERROR"],
        )
        timed = run(
            "timeout",
            table,
            [Check(age, "deadline", on_error="WARN")],
            mode="timeout",
            expected=["ERROR"],
        )
        assert timed["result"]["checks"][0]["error"] == "validation_timeout", timed
        assert "input validation" in timed["worker"]["timeout"], timed
        large = pa.table(
            {"id": pa.array((i * 1_000_000_007 for i in range(2_000_000)), pa.int64())}
        )
        spilled = run(
            "spill",
            large,
            [Check(E.primary_key("id"), "pk")],
            memory="32000000",
            spill="536870912",
            expected=["PASS"],
        )
        assert spilled["measurement"]["peak_bytes"] > 0, spilled
        run(
            "disk-error",
            large,
            [Check(E.primary_key("id"), "pk", on_error="WARN")],
            memory="32000000",
            spill="0",
            expected=["ERROR"],
        )
    print(
        json.dumps(
            {
                "result": "pass",
                "checks": cases,
                "duckdb": duckdb.__version__,
                "group_spill_peak_bytes": spilled["measurement"]["peak_bytes"],
            }
        )
    )


if __name__ == "__main__":
    main()
