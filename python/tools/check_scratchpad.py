"""Qualify the production SQL policy and installed isolated helper with real pinned engines."""

from __future__ import annotations

import argparse
import hashlib
import importlib
import json
import os
import subprocess
import sys
import tempfile
import venv
from decimal import Decimal
from pathlib import Path
from typing import Any
from uuid import uuid4

REPO = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(REPO / "python/sdk/src"), str(REPO / "python/worker/src")]
from transflow_worker.canonical import artifact_digest, schema_digest  # noqa: E402
from transflow_worker.polars_adapter import logical_schema  # noqa: E402
from transflow_worker.scratchpad import _connect  # noqa: E402
from transflow_worker.sql_policy import SqlPolicyError, validate_query  # noqa: E402


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("wheel", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    wheel = args.wheel.resolve(strict=True)
    pl = importlib.import_module("polars")
    pa = importlib.import_module("pyarrow")
    pq = importlib.import_module("pyarrow.parquet")
    cases: list[str] = []
    with tempfile.TemporaryDirectory(prefix="tf-scratchpad-") as temporary:
        root = Path(temporary).resolve()
        interpreter = root / "venv/bin/python"
        venv.EnvBuilder(with_pip=False).create(interpreter.parent.parent)
        subprocess.run(
            [
                sys.executable,
                "-m",
                "pip",
                "--python",
                str(interpreter),
                "install",
                "--no-index",
                "--no-deps",
                "--force-reinstall",
                "--only-binary=:all:",
                "--find-links",
                str(REPO / "target/qualification/wheelhouse"),
                str(wheel),
                "duckdb==1.5.5",
                "pyarrow==25.0.1",
            ],
            check=True,
            capture_output=True,
        )
        artifact = root / "provider"
        artifact.mkdir()
        frame = pl.DataFrame(
            {
                "id": [1, 2, 3],
                "group": ["a", "a", "b"],
                "amount": [10, 20, 30],
                "note": ["first", None, "last"],
            }
        )
        source = artifact / "part-0.parquet"
        frame.write_parquet(source)
        schema = logical_schema(pl, frame.schema)
        manifest: dict[str, Any] = {
            "format_version": 1,
            "logical_schema": schema,
            "schema_fingerprint": schema_digest(schema).hex,
            "files": [
                {
                    "path": source.name,
                    "sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
                    "byte_length": str(source.stat().st_size),
                    "row_count": "3",
                }
            ],
            "writer": {
                "engine": "polars",
                "version": pl.__version__,
                "compression": "zstd",
                "row_group_size": "3",
            },
        }
        binding = {
            "alias": "dataset",
            "workspace_id": str(uuid4()),
            "dataset_id": str(uuid4()),
            "version_id": str(uuid4()),
            "artifact_digest": artifact_digest(manifest).hex,
            "artifact_root": str(artifact),
            "manifest": manifest,
        }
        outside = root / "outside.txt"
        outside.write_text("private-synthetic-sentinel")
        source_before = source.read_bytes()
        # Poison both a module and sitecustomize in the helper's cwd. -I must ignore them.
        (root / "transflow_worker.py").write_text("raise RuntimeError('producer imported')")
        (root / "sitecustomize.py").write_text("raise RuntimeError('cwd hook imported')")

        def helper(
            sql: str,
            *,
            parameters: list[Any] | None = None,
            succeeds: bool = True,
            override: dict[str, Any] | None = None,
        ) -> tuple[Any, dict[str, Any]] | None:
            directory = root / str(uuid4())
            directory.mkdir(mode=0o700)
            results = directory / "results"
            results.mkdir(mode=0o700)
            request = {
                "format_version": 1,
                "request_id": str(uuid4()),
                "sql": sql,
                "parameters": parameters or [],
                "bindings": [binding],
                "result_directory": str(results),
                **(override or {}),
            }
            request_path = directory / "request.json"
            request_path.write_text(json.dumps(request))
            request_path.chmod(0o600)
            completed = subprocess.run(
                [
                    str(interpreter),
                    "-I",
                    "-B",
                    "-m",
                    "transflow_worker.scratchpad",
                    str(request_path),
                ],
                cwd=root,
                env={"PATH": os.defpath, "HOME": str(directory), "TMPDIR": str(directory)},
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
            )
            assert completed.stdout == "", completed.stdout
            assert not list(results.glob("spill-*")), "Query spill was retained"
            if not succeeds:
                assert completed.returncode != 0, sql
                assert not list(results.iterdir()), "Failed query left a usable result"
                assert str(root) not in completed.stderr and sql not in completed.stderr
                return None
            assert completed.returncode == 0, completed.stderr
            metadata = json.loads((results / "result.json").read_text())
            assert metadata["request_id"] == request["request_id"]
            assert metadata["bindings"][0]["version_id"] == request["bindings"][0]["version_id"]
            with pa.ipc.open_stream(results / "result.arrow") as reader:
                table = reader.read_all()  # Only the helper's capped result, never input data.
            assert table.num_rows == int(metadata["row_count"])
            assert (results / "result.arrow").stat().st_size <= 2 * 1024 * 1024
            assert {p.name for p in results.iterdir()} == {"result.arrow", "result.json"}
            return table, metadata

        positives = [
            (
                'SELECT "group", sum(amount) AS total FROM dataset GROUP BY "group" ORDER BY 1',
                [{"group": "a", "total": 30}, {"group": "b", "total": 30}],
            ),
            (
                'WITH totals AS (SELECT "group", sum(amount) total FROM dataset GROUP BY "group") '
                "SELECT total FROM totals WHERE total > 0 ORDER BY total",
                [{"total": 30}, {"total": 30}],
            ),
            (
                "SELECT a.id FROM dataset a JOIN dataset b ON a.id=b.id "
                "WHERE b.amount BETWEEN 15 AND 25",
                [{"id": 2}],
            ),
            (
                "SELECT id, row_number() OVER (ORDER BY id) AS n FROM dataset ORDER BY id",
                [{"id": 1, "n": 1}, {"id": 2, "n": 2}, {"id": 3, "n": 3}],
            ),
            (
                "SELECT id FROM dataset WHERE EXISTS "
                "(SELECT 1 FROM dataset b WHERE b.id=dataset.id) "
                "ORDER BY id LIMIT 1",
                [{"id": 1}],
            ),
            ("SELECT id FROM dataset WHERE note IS NULL", [{"id": 2}]),
            (
                "SELECT id FROM dataset WHERE id IN (SELECT id FROM dataset WHERE amount=30)",
                [{"id": 3}],
            ),
            ("SELECT count(*) AS n FROM dataset WHERE false", [{"n": 0}]),
            ("SELECT id FROM dataset WHERE false", []),
            (
                '/* a comment */ SELECT DISTINCT upper("group") AS g FROM dataset ORDER BY g;',
                [{"g": "A"}, {"g": "B"}],
            ),
        ]
        for sql, expected in positives:
            actual = helper(sql)
            assert actual is not None and actual[0].to_pylist() == expected
        cases.append(f"{len(positives)} analytical SELECT/WITH/group/join/window/subquery fixtures")
        result = helper(
            "SELECT id FROM dataset WHERE amount > CAST(? AS BIGINT) ORDER BY id", parameters=["20"]
        )
        assert result is not None and result[0].to_pylist() == [{"id": 3}]
        result = helper(
            "SELECT CAST(? AS VARCHAR) AS literal FROM dataset LIMIT 1",
            parameters=["'; COPY dataset TO 'escape'; --"],
        )
        assert result is not None and result[0].column(0)[0].as_py().startswith("'; COPY")
        cases.append("bound parameters remain data")
        result = helper(
            "SELECT count(*) AS n FROM dataset JOIN other ON dataset.id < other.id",
            override={"bindings": [binding, {**binding, "alias": "other"}]},
        )
        assert result is not None and result[0].to_pylist() == [{"n": 3}]
        assert len(result[1]["bindings"]) == 2
        cases.append("additional explicit version views retain separate binding identities")

        negatives = [
            "",
            "SELECT 1; SELECT 2",
            "SELECT 1; COPY dataset TO 'escape'",
            "CREATE TABLE x(i INT)",
            "CREATE MACRO x() AS 1",
            "CREATE SECRET x (TYPE S3)",
            "INSERT INTO dataset VALUES (9)",
            "DELETE FROM dataset",
            "UPDATE dataset SET id=9",
            "DROP VIEW dataset",
            "ALTER VIEW dataset RENAME TO renamed",
            "VACUUM",
            "ATTACH 'outside.db' AS x",
            "PRAGMA version",
            "SET enable_external_access=true",
            "INSTALL httpfs",
            "LOAD httpfs",
            "CALL checkpoint()",
            f"COPY dataset TO '{source}' (FORMAT PARQUET, USE_TMP_FILE false)",
            f"SELECT * FROM '{source}'",
            f"SELECT * FROM read_parquet('{source}')",
            f"SELECT * FROM read_text('{outside}')",
            "SELECT * FROM read_csv('https://example.invalid/x')",
            "SELECT * FROM glob('*')",
            "SELECT * FROM range(100)",
            "SELECT * FROM query('SELECT 1')",
            "SELECT * FROM duckdb_settings()",
            "SELECT * FROM information_schema.tables",
            "SELECT * FROM main.dataset",
            "SELECT * FROM missing",
            "SELECT getenv('HOME')",
            "SELECT current_setting('allowed_paths')",
            "SELECT nextval('counter')",
            "SELECT setseed(0.1)",
            "SELECT repeat('x', 100000000)",
            "SELECT sum(amount) FROM dataset UNION ALL SELECT 1",
            "WITH RECURSIVE x AS (SELECT 1 n UNION ALL SELECT n+1 FROM x) SELECT * FROM x",
            "SELECT COLUMNS(*) FROM dataset",
            "SELECT * FROM dataset USING SAMPLE 1",
            "SELECT CAST(id AS custom_type) FROM dataset",
            "WITH x AS (SELECT * FROM read_text('outside')) SELECT * FROM x",
            "SELECT * FROM dataset WHERE EXISTS(SELECT * FROM read_text('outside'))",
        ]
        # Test policy rejection itself, not a coincidental binder or engine permission error.
        spill = root / "engine-spill"
        spill.mkdir(mode=0o700)
        with _connect({"dataset": [str(source)]}, spill) as db:
            for sql in negatives:
                try:
                    validate_query(db, sql, ["dataset"])
                except SqlPolicyError:
                    pass
                else:
                    raise AssertionError(f"Policy accepted forbidden query: {sql}")
            for sql in [
                "SET lock_configuration=false",
                "SET enable_external_access=true",
                f"SELECT * FROM read_text('{outside}')",
                "LOAD httpfs",
                "SELECT * FROM read_csv('https://example.invalid/x')",
            ]:
                try:
                    db.execute(sql)
                except Exception:
                    pass
                else:
                    raise AssertionError("Locked engine allowed forbidden operation")
            settings = dict(
                db.execute(
                    "SELECT name,value FROM duckdb_settings() WHERE name IN "
                    "('allowed_directories','lock_configuration')"
                ).fetchall()
            )
            assert settings["lock_configuration"] == "true"
            assert settings["allowed_directories"] == f"[{spill}/]"
        bounded_spill = root / "bounded-engine-spill"
        bounded_spill.mkdir(mode=0o700)
        with _connect(
            {"dataset": [str(source)]},
            bounded_spill,
            {
                "rows": 1000,
                "bytes": 2097152,
                "threads": 1,
                "memory_bytes": "8388608",
                "spill_bytes": "0",
            },
        ) as db:
            # Trusted engine probe, outside the authored-SQL allowlist. A tiny output
            # still requires a large sort; zero disk forbids spill instead of growing it.
            assert db.execute("SELECT current_setting('threads')").fetchone() == (1,)
            try:
                db.execute("SELECT i FROM range(5000000) t(i) ORDER BY i % 10000, i").fetchone()
            except importlib.import_module("duckdb").OutOfMemoryException:
                pass
            else:
                raise AssertionError("Sort exceeded its memory/zero-spill allocation")
        assert not any(bounded_spill.iterdir())
        cases.append(
            "explicit threads/memory and zero spill stop computation independent of output"
        )
        for sql in negatives[1::5]:
            helper(sql, succeeds=False)
        helper(f"COPY dataset TO '{source}' (FORMAT PARQUET, USE_TMP_FILE false)", succeeds=False)
        cases.append(f"{len(negatives)} AST denials and five engine defence-in-depth denials")
        assert source.read_bytes() == source_before
        assert outside.read_text() == "private-synthetic-sentinel"
        cases.append("write denial preserves provider and outside bytes; no producer/cwd imports")
        helper("SELECT error('private-synthetic-sentinel')", succeeds=False)
        helper("SELECT missing_column FROM dataset", succeeds=False)
        bad_binding = {**binding, "artifact_digest": "0" * 64}
        helper("SELECT * FROM dataset", succeeds=False, override={"bindings": [bad_binding]})
        helper("SELECT * FROM dataset", succeeds=False, override={"bindings": [binding, binding]})
        cases.append("private failures, corrupt bindings and duplicate aliases fail closed")
        # Unsupported precision must fail before a query could silently coerce it.
        for table in [
            pa.table({"aware": pa.array([1], type=pa.timestamp("ns", tz="UTC"))}),
            pa.table({"nested": [[1, 2]]}),
            pa.table({"ID": [1], "id": [2]}),
        ]:
            pq.write_table(table, source)
            manifest["files"][0].update(
                sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
                byte_length=str(source.stat().st_size),
            )
            binding["artifact_digest"] = artifact_digest(manifest).hex
            helper("SELECT * FROM dataset", succeeds=False)
        cases.append("lossy timestamp, nested input and case-colliding fields refused")

        def replace_input(table: Any) -> None:
            pq.write_table(table, source)
            manifest["logical_schema"] = logical_schema(pl, pl.from_arrow(table).schema)
            manifest["schema_fingerprint"] = schema_digest(manifest["logical_schema"]).hex
            manifest["files"][0].update(
                sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
                byte_length=str(source.stat().st_size),
                row_count=str(table.num_rows),
            )
            binding["artifact_digest"] = artifact_digest(manifest).hex
            binding["version_id"] = str(uuid4())

        exact = pa.table(
            {
                "wide": pa.array([2**64 - 1, None], type=pa.uint64()),
                "precise": pa.array(
                    [Decimal("123456789012345678901234567890.1234"), None],
                    type=pa.decimal128(38, 4),
                ),
                "instant": pa.array([1234567890123456789, None], type=pa.timestamp("ns")),
            }
        )
        replace_input(exact)
        result = helper("SELECT * FROM dataset")
        assert result is not None and result[0].equals(exact)
        cases.append("Arrow result retains exact u64, precision-38 decimal, ns timestamp and null")
        replace_input(pa.table({"id": list(range(2200))}))
        result = helper("SELECT id FROM dataset ORDER BY id")
        assert result is not None and result[0].num_rows == 1000 and result[1]["truncated"]
        replace_input(pa.table({"large": ["x" * (2 * 1024 * 1024 + 1)]}))
        result = helper("SELECT large FROM dataset")
        assert result is not None and result[0].num_rows == 0 and result[1]["truncated"]
        cases.append("row and encoded-byte output caps report truncation without publishing")
        replace_input(pa.table({"id": [1]}))
        second = artifact / "part-1.parquet"
        pq.write_table(pa.table({"id": [2]}), second)
        manifest["files"].append(
            {
                "path": second.name,
                "sha256": hashlib.sha256(second.read_bytes()).hexdigest(),
                "byte_length": str(second.stat().st_size),
                "row_count": "1",
            }
        )
        binding["artifact_digest"] = artifact_digest(manifest).hex
        result = helper("SELECT id FROM dataset ORDER BY id")
        assert result is not None and result[0].to_pylist() == [{"id": 1}, {"id": 2}]
        source.write_bytes(source.read_bytes() + b"changed")
        helper("SELECT id FROM dataset", succeeds=False)
        cases.append("exact multi-file manifests work and changed provider bytes fail")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(
            {
                "result": "pass",
                "cases": cases,
                "python": sys.version.split()[0],
                "duckdb": importlib.import_module("duckdb").__version__,
                "pyarrow": pa.__version__,
            },
            indent=2,
        )
        + "\n"
    )
    print(f"SQL scratchpad qualification passed: {len(cases)} scenario groups")


if __name__ == "__main__":
    main()
