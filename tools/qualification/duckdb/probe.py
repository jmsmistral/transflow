"""Supervised, offline DuckDB 1.5.5 capability spike. No installed Transflow imports."""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import math
import os
import platform
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from decimal import Decimal
from pathlib import Path

import duckdb
import pandas as pd
import polars as pl
import pyarrow as pa
import pyarrow.parquet as pq
from restricted import Unsupported, canonical_file, connect, scratchpad, validate_schema

OBSERVATIONS = {}


def checksum(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class CapabilityTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="case-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.path = self.root / "candidate's data.parquet"
        self.outside = self.root / "unregistered.parquet"
        self.table = pa.table(
            {
                "id": pa.array([2**53 + 1, -(2**63), None, 7], type=pa.int64()),
                "amount": pa.array(
                    [
                        Decimal("12345678901234567890.1234"),
                        Decimal("-0.0001"),
                        None,
                        Decimal("1.0000"),
                    ],
                    type=pa.decimal128(38, 4),
                ),
                "event_time": pa.array(
                    [1710000000000000001, -1, None, 1710000000000000003], type=pa.timestamp("ns")
                ),
                "reading": pa.array([float("nan"), None, 2.5, -1.0], type=pa.float64()),
            }
        )
        pq.write_table(self.table, self.path)
        pq.write_table(pa.table({"id": [999]}), self.outside)
        self.original = checksum(self.path)
        self.metadata = {}
        self.db = connect({"dataset": self.path}, self.root / "spill", metadata=self.metadata)
        self.addCleanup(self.db.close)

    def test_exact_registered_scan_and_locked_configuration(self):
        settings = dict(self.db.execute("SELECT name,value FROM duckdb_settings()").fetchall())
        for key in (
            "enable_external_access",
            "autoinstall_known_extensions",
            "autoload_known_extensions",
            "allow_community_extensions",
            "allow_unsigned_extensions",
            "python_enable_replacements",
            "allow_persistent_secrets",
        ):
            self.assertEqual(settings[key], "false", key)
        self.assertEqual(
            self.db.execute("SELECT current_setting('allowed_directories')").fetchone()[0],
            [str(self.root / "spill") + "/"],
        )
        OBSERVATIONS["engine_adds_only_owned_spill_directory"] = True
        self.assertEqual(settings["allowed_configs"], "[]")
        self.assertEqual(
            self.db.execute("SELECT current_setting('allowed_paths')").fetchone()[0],
            [str(self.path)],
        )
        self.assertEqual(settings["lock_configuration"], "true")
        self.assertEqual(self.db.execute("SELECT count(*) FROM dataset").fetchone(), (4,))
        self.assertEqual(checksum(self.path), self.original)
        OBSERVATIONS["engine_default_memory_limit"] = settings["memory_limit"]
        OBSERVATIONS["required_bundled_extensions"] = self.metadata["bundled_extensions"]
        self.assertEqual(
            {name for (name,) in self.metadata["bundled_extensions"]},
            {"core_functions", "icu", "json", "parquet"},
        )

    def test_ambient_python_tables_are_not_visible(self):
        ambient_table = pa.table({"value": [123]})
        self.assertEqual(ambient_table.num_rows, 1)
        with self.assertRaises(duckdb.CatalogException):
            self.db.execute("SELECT * FROM ambient_table")

    def test_matching_metrics_on_three_engine_materializations(self):
        paths = {name: self.root / f"{name}.parquet" for name in ("polars", "pandas", "sql")}
        pl.from_arrow(self.table).lazy().sink_parquet(paths["polars"])
        self.table.to_pandas(types_mapper=pd.ArrowDtype).to_parquet(
            paths["pandas"], engine="pyarrow", index=False
        )
        with duckdb.connect() as transform:
            transform.execute(
                "COPY (SELECT * FROM read_parquet($source)) TO $output (FORMAT PARQUET)",
                {"source": str(self.path), "output": str(paths["sql"])},
            )
        metrics = []
        for name, path in paths.items():
            validate_schema(pq.read_schema(path))
            with connect({"dataset": path}, self.root / f"{name}-spill") as evaluator:
                metrics.append(
                    evaluator.execute(
                        "SELECT count(*), sum(id), sum(amount), "
                        "count(*) FILTER (WHERE isnan(reading)), "
                        "sum(epoch_ns(event_time)) FROM dataset"
                    ).fetchone()
                )
        self.assertEqual(metrics[0], metrics[1])
        self.assertEqual(metrics[0], metrics[2])
        OBSERVATIONS["equivalent_engine_metrics"] = ["polars", "pandas_arrow_backed", "duckdb_sql"]

    def test_outside_files_network_writes_and_external_extensions_denied(self):
        output = self.root / "forbidden.parquet"
        cases = [
            ("outside_parquet", "SELECT * FROM read_parquet(?)", [str(self.outside)]),
            ("outside_blob", "SELECT * FROM read_blob(?)", [str(self.outside)]),
            ("glob", "SELECT * FROM glob(?)", [str(self.root / "*.parquet")]),
            (
                "network_http",
                "SELECT * FROM read_parquet(?)",
                ["http://127.0.0.1:9/blocked.parquet"],
            ),
            (
                "network_s3",
                "SELECT * FROM read_parquet(?)",
                ["s3://transflow-synthetic-denied/blocked.parquet"],
            ),
            ("copy_outside", "COPY dataset TO ? (FORMAT PARQUET)", [str(output)]),
            (
                "attach",
                "ATTACH '"
                + str(self.root / "blocked.duckdb").replace("'", "''")
                + "' AS external_db",
                [],
            ),
            ("install", "INSTALL httpfs", []),
            ("load", "LOAD httpfs", []),
        ]
        failures = {}
        for label, sql, parameters in cases:
            with self.subTest(label=label):
                with self.assertRaises(duckdb.PermissionException) as error:
                    self.db.execute(sql, parameters)
                failures[label] = type(error.exception).__name__
        self.assertFalse(output.exists())
        self.assertFalse((self.root / "blocked.duckdb").exists())
        self.assertFalse((self.root / "extensions").exists())
        self.assertEqual(checksum(self.path), self.original)
        OBSERVATIONS["engine_denials"] = failures

    def test_configuration_cannot_be_reenabled(self):
        for setting in (
            "enable_external_access=true",
            "allowed_paths=[]",
            "allowed_directories=[]",
            "autoload_known_extensions=true",
            "autoinstall_known_extensions=true",
            "allow_community_extensions=true",
            "allow_unsigned_extensions=true",
            "memory_limit='1TB'",
            "lock_configuration=false",
            "allowed_configs=['memory_limit']",
        ):
            with self.subTest(setting=setting), self.assertRaises(duckdb.Error):
                self.db.execute("SET " + setting)

    def test_parsed_select_cte_join_window_and_parameters(self):
        queries = [
            (
                "WITH picked AS (SELECT id FROM dataset WHERE id > $minimum) "
                "SELECT count(*) FROM picked",
                {"minimum": 0},
                [(2,)],
            ),
            (
                "SELECT a.id, row_number() OVER (ORDER BY a.id) AS n FROM dataset a "
                "JOIN dataset b ON a.id=b.id WHERE a.id>0 ORDER BY a.id",
                {},
                [(7, 1), (2**53 + 1, 2)],
            ),
            (
                "SELECT count(*) FROM dataset WHERE 'quote''; DROP TABLE dataset; --' = $value",
                {"value": "quote'; DROP TABLE dataset; --"},
                [(4,)],
            ),
        ]
        for sql, parameters, expected in queries:
            with self.subTest(sql=sql):
                rows, truncated = scratchpad(self.db, sql, {"dataset"}, parameters)
                self.assertEqual(rows, expected)
                self.assertFalse(truncated)
        alias = 'selected "input"'
        with connect({alias: self.path}, self.root / "other-spill") as db:
            self.assertEqual(
                scratchpad(db, 'SELECT count(*) FROM "selected ""input"""', {alias})[0], [(4,)]
            )

    def test_scratchpad_rejects_nonqueries_and_indirect_io(self):
        denied = [
            "SELECT 1; SELECT 2",
            "CREATE TABLE evil(id INTEGER)",
            "DELETE FROM dataset",
            "PRAGMA version",
            "SET enable_external_access=true",
            "ATTACH 'other.db' AS other",
            "COPY dataset TO 'other.parquet'",
            "INSTALL httpfs",
            "LOAD httpfs",
            "CREATE MACRO evil() AS 1",
            "SELECT * FROM read_parquet('outside.parquet')",
            "WITH x AS (SELECT * FROM read_csv('outside.csv')) SELECT * FROM x",
            "SELECT (SELECT content FROM read_blob('outside')) FROM dataset",
            "SELECT * FROM query('SELECT * FROM dataset')",
            "SELECT * FROM duckdb_settings()",
            "SELECT * FROM information_schema.tables",
            "SELECT * FROM 'outside.parquet'",
            "SELECT getenv('HOME')",
            "SELECT current_setting('secret_directory')",
            "SELECT * FROM range(1000000000)",
            "SELECT md5('outside-reviewed-functions')",
            "SELECT 1 UNION SELECT 2",
        ]
        for sql in denied:
            with self.subTest(sql=sql), self.assertRaises(Unsupported):
                scratchpad(self.db, sql, {"dataset"})
        self.assertEqual(checksum(self.path), self.original)
        OBSERVATIONS["parsed_policy_denials"] = len(denied)

    def test_file_allowlist_is_not_a_readonly_permission(self):
        # A sacrificial registered file proves why engine settings cannot replace AST validation.
        disposable = self.root / "sacrificial.parquet"
        pq.write_table(pa.table({"id": [1]}), disposable)
        with connect({"dataset": disposable}, self.root / "write-control-spill") as db:
            sql = "COPY (SELECT 2 AS id) TO ? (FORMAT PARQUET, USE_TMP_FILE false)"
            before = checksum(disposable)
            with self.assertRaises(Unsupported):
                scratchpad(db, sql, {"dataset"}, [str(disposable)])
            self.assertEqual(checksum(disposable), before)
            db.execute(sql, [str(disposable)])  # Trusted negative control on synthetic bytes only.
        self.assertEqual(pq.read_table(disposable)["id"].to_pylist(), [2])
        OBSERVATIONS["allowlisted_raw_copy_can_write"] = True
        OBSERVATIONS["scratchpad_ast_blocks_allowlisted_copy"] = True

    def test_exact_canonical_aggregates(self):
        validate_schema(self.table.schema)
        # These expressions represent application-compiled predicates, never user-provided SQL.
        row = self.db.execute(
            """SELECT count(*), count(id), count(*) FILTER (WHERE id IS NULL),
            count(*) FILTER (WHERE (id > ?) IS TRUE), count(*) FILTER (WHERE (id > ?) IS NOT TRUE),
            sum(id), sum(amount), min(amount), max(amount),
            count(*) FILTER (WHERE isnan(reading)), count(*) FILTER (WHERE reading IS NULL)
            FROM dataset""",
            [0, 0],
        ).fetchone()
        self.assertEqual(
            row,
            (
                4,
                3,
                1,
                2,
                2,
                2**53 + 1 - 2**63 + 7,
                Decimal("12345678901234567891.1233"),
                Decimal("-0.0001"),
                Decimal("12345678901234567890.1234"),
                1,
                1,
            ),
        )
        empty = self.db.execute(
            "SELECT count(*), sum(amount), min(amount), max(amount) FROM dataset WHERE false"
        ).fetchone()
        self.assertEqual(empty, (0, None, None, None))
        OBSERVATIONS["exact_aggregates"] = {
            "rows": row[0],
            "integer_sum": str(row[5]),
            "decimal_sum": str(row[6]),
            "nan_count": row[9],
            "null_count": row[10],
        }

    def test_arrow_roundtrip_and_python_timestamp_loss(self):
        back = self.db.execute("SELECT * FROM dataset").to_arrow_table()
        for name in ("id", "amount", "event_time"):
            self.assertTrue(back[name].equals(self.table[name]), name)
        values = back["reading"].to_pylist()
        self.assertTrue(math.isnan(values[0]))
        self.assertIsNone(values[1])
        python_time, exact_ns = self.db.execute(
            "SELECT event_time, epoch_ns(event_time) FROM dataset WHERE id>100"
        ).fetchone()
        self.assertEqual(exact_ns, 1710000000000000001)
        self.assertEqual(python_time.microsecond, 0)
        roundtrip = self.root / "roundtrip.parquet"
        pq.write_table(back, roundtrip)
        self.assertTrue(pq.read_table(roundtrip)["event_time"].equals(self.table["event_time"]))
        OBSERVATIONS["timestamp_transport"] = {
            "naive_arrow_ns_exact": True,
            "python_datetime_loses_submicroseconds": True,
        }

    def test_timezone_precision_and_wide_decimals_are_guarded(self):
        aware = pa.table(
            {"event_time": pa.array([1710000000000000001], type=pa.timestamp("ns", tz="UTC"))}
        )
        with self.assertRaisesRegex(Unsupported, "lose precision"):
            validate_schema(aware.schema)
        path = self.root / "aware.parquet"
        pq.write_table(aware, path)
        with connect({"dataset": path}, self.root / "aware-spill") as db:
            actual = db.execute("SELECT epoch_ns(event_time) FROM dataset").fetchone()[0]
        self.assertEqual(actual, 1710000000000000000)
        micro = pa.table(
            {"event_time": pa.array([1710000000000001], type=pa.timestamp("us", tz="UTC"))}
        )
        validate_schema(micro.schema)
        micro_path = self.root / "micro.parquet"
        pq.write_table(micro, micro_path)
        with connect({"dataset": micro_path}, self.root / "micro-spill") as db:
            self.assertEqual(
                db.execute("SELECT epoch_us(event_time) FROM dataset").fetchone(),
                (1710000000000001,),
            )
        wide = pa.table({"amount": pa.array([Decimal("1" * 40)], type=pa.decimal256(40, 0))})
        with self.assertRaisesRegex(Unsupported, "38 digits"):
            validate_schema(wide.schema)
        wide_path = self.root / "wide.parquet"
        pq.write_table(wide, wide_path)
        with connect({"dataset": wide_path}, self.root / "wide-spill") as db:
            cursor = db.execute("SELECT amount FROM dataset")
            raw_type = str(cursor.description[0][1])
            raw_value = cursor.fetchone()[0]
        OBSERVATIONS["unsupported_types"] = {
            "timezone_ns_input": "1710000000000000001",
            "timezone_ns_engine_output": str(actual),
            "decimal40_engine_type": raw_type,
            "decimal40_engine_value": str(raw_value),
            "preflight_rejects_both": True,
        }

    def test_decimal_average_and_overflow(self):
        cursor = self.db.execute("SELECT avg(amount), sum(amount) FROM dataset")
        self.assertEqual(str(cursor.description[0][1]), "DOUBLE")
        self.assertEqual(str(cursor.description[1][1]), "DECIMAL(38,4)")
        with self.assertRaises(duckdb.OutOfRangeException):
            self.db.execute(
                "SELECT sum(amount) FROM (VALUES (CAST(? AS DECIMAL(38,0))), "
                "(CAST(? AS DECIMAL(38,0)))) t(amount)",
                ["9" * 38, "9" * 38],
            ).fetchone()
        OBSERVATIONS["decimal_aggregate_limits"] = {
            "avg_type": "DOUBLE",
            "sum_overflow": "OutOfRangeException",
        }

    def test_unsigned_integer_exactness(self):
        table = pa.table({"value": pa.array([2**64 - 1, 2**64 - 2, None], type=pa.uint64())})
        path = self.root / "unsigned.parquet"
        pq.write_table(table, path)
        validate_schema(table.schema)
        with connect({"dataset": path}, self.root / "unsigned-spill") as db:
            self.assertEqual(
                db.execute("SELECT sum(value), max(value) FROM dataset").fetchone(),
                (2**65 - 3, 2**64 - 1),
            )
            self.assertTrue(
                db.execute("SELECT * FROM dataset").to_arrow_table()["value"].equals(table["value"])
            )

    def test_symlink_and_missing_binding_rejected(self):
        alias = self.root / "link.parquet"
        alias.symlink_to(self.path)
        with self.assertRaisesRegex(Unsupported, "Symlink"):
            canonical_file(alias)
        with self.assertRaises(Unsupported):
            canonical_file(self.root / "missing.parquet")

    def test_interruption_of_observed_active_query(self):
        finished = threading.Event()
        outcomes = []

        def run():
            try:
                self.db.execute(
                    "SELECT sum(a.range+b.range) FROM range(1000000000) a, range(1000000000) b"
                ).fetchone()
                outcomes.append("unexpected completion")
            except duckdb.Error as error:
                outcomes.append(type(error).__name__)
            finally:
                finished.set()

        worker = threading.Thread(target=run, daemon=True)
        worker.start()
        deadline = time.monotonic() + 5
        try:
            while (
                self.db.query_progress() < 0
                and time.monotonic() < deadline
                and not finished.wait(0.001)
            ):
                pass  # Poll observable engine progress, not a sleep used to assume query startup.
            self.assertGreaterEqual(self.db.query_progress(), 0)
            started = time.monotonic()
            self.db.interrupt()
            self.assertTrue(finished.wait(5), "Active query did not stop after interruption")
            self.assertEqual(outcomes, ["InterruptException"])
            OBSERVATIONS["interrupt"] = {
                "active_query_observed": True,
                "seconds": time.monotonic() - started,
            }
        finally:
            self.db.interrupt()
            worker.join(5)
        self.assertFalse(worker.is_alive())
        self.assertEqual(self.db.execute("SELECT count(*) FROM dataset").fetchone(), (4,))
        self.assertEqual(checksum(self.path), self.original)

    def test_spill_and_temporary_budget_failure(self):
        path = self.root / "million.parquet"
        with (
            duckdb.connect() as writer
        ):  # Trusted fixture generation, separate from restricted helpers.
            writer.execute(
                "COPY (SELECT range AS id FROM range(1000000)) TO ? (FORMAT PARQUET)", [str(path)]
            )
        query = "SELECT id FROM dataset ORDER BY md5(CAST(id AS VARCHAR))"
        spill = self.root / "sort-spill"
        with connect({"dataset": path}, spill, memory_limit="32MB") as db:
            db.execute(query)
            spill_bytes = sum(p.stat().st_size for p in spill.iterdir() if p.is_file())
            self.assertGreater(spill_bytes, 0, "No physical disk spill was observed")
            count = 0
            while batch := db.fetchmany(4096):
                count += len(batch)
            self.assertEqual(count, 1000000)
        self.assertFalse(
            spill.exists() and any(spill.iterdir()), "Spill files survived connection close"
        )
        denied_spill = self.root / "budget-spill"
        with connect({"dataset": path}, denied_spill, memory_limit="32MB", spill_limit="0B") as db:
            with self.assertRaises(duckdb.OutOfMemoryException):
                db.execute(query)
        self.assertFalse(denied_spill.exists() and any(denied_spill.iterdir()))
        with connect({"dataset": path}, self.root / "bounded-spill") as db:
            rows, truncated = scratchpad(db, "SELECT id FROM dataset", {"dataset"})
            self.assertEqual(len(rows), 1000)
            self.assertTrue(truncated)
        OBSERVATIONS["spill"] = {
            "rows_drained_in_4096_row_batches": count,
            "observed_bytes": spill_bytes,
            "test_memory_limit": "32MB",
            "temp_budget": "256MB",
            "files_removed_on_close": True,
            "zero_temp_budget": "OutOfMemoryException",
        }


def run_child():
    expected = {"duckdb": "1.5.5", "pyarrow": "25.0.1", "polars": "1.44.2", "pandas": "3.0.6"}
    versions = {name: importlib.metadata.version(name) for name in expected}
    if versions != expected or sys.version_info[:2] != (3, 14):
        raise RuntimeError(
            f"Unqualified probe environment: {versions}; Python {platform.python_version()}"
        )
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(CapabilityTests)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    report = {
        "task": "T009",
        "python": platform.python_version(),
        "platform": platform.system(),
        "machine": platform.machine(),
        "packages": versions,
        "tests": result.testsRun,
        "failures": len(result.failures),
        "errors": len(result.errors),
        "observations": OBSERVATIONS,
        "scope": "Feasibility only; not the T061 evaluator or T079 scratchpad service",
        "limits": [
            "No hostile SQL sandbox claim",
            "Prototype SELECT AST/function subset only",
            "No full logical-type conformance",
            "No byte-budgeted result protocol or version leases",
            "Resource settings in spill tests are test-only, not application defaults",
        ],
    }
    print(json.dumps(report, indent=2))
    return 0 if result.wasSuccessful() else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--child", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.child:
        return run_child()
    if args.output is None:
        parser.error("--output is required")
    with tempfile.TemporaryDirectory(prefix="transflow-duckdb-helper-") as directory:
        root = Path(directory).resolve()
        environment = {
            "PATH": os.defpath,
            "HOME": str(root),
            "TMPDIR": str(root),
            "PYTHONNOUSERSITE": "1",
            "PYTHONDONTWRITEBYTECODE": "1",
        }
        # This fixed child emits bounded fixture/test metadata, never arbitrary user query output.
        result = subprocess.run(
            [sys.executable, "-B", str(Path(__file__).resolve()), "--child"],
            cwd=root,
            env=environment,
            text=True,
            capture_output=True,
            timeout=90,
            check=False,
        )
    sys.stderr.write(result.stderr)
    if result.returncode:
        sys.stderr.write(result.stdout)
        return result.returncode
    report = json.loads(result.stdout)
    report["supervision"] = {
        "source_environment_inherited": False,
        "timeout_seconds": 90,
        "helper_reaped": True,
        "owned_directory_removed": not root.exists(),
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"DuckDB capability probe passed: {report['tests']} tests; report: {args.output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
