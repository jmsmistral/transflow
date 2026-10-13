"""Real SQL worker, Rust artifact service and exact Parquet boundary qualification."""

from __future__ import annotations

import hashlib
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
from transflow.declarations import get_declaration  # noqa: E402
from transflow_worker.canonical import catalog_fingerprint  # noqa: E402
from transflow_worker.discovery import _check, _declaration  # noqa: E402
from transflow_worker.source_index import ModuleIndex  # noqa: E402


def main() -> None:
    probe, normalization, wheel = map(lambda p: Path(p).resolve(strict=True), sys.argv[1:4])
    pa, pq = importlib.import_module("pyarrow"), importlib.import_module("pyarrow.parquet")
    checks = []
    check_probe = str(Path(sys.argv[4]).resolve(strict=True)) if len(sys.argv) > 4 else None
    matrix_evaluations: dict[str, Any] = {}

    def command(args: list[str]) -> dict[str, Any]:
        result = subprocess.run(args, capture_output=True, text=True, timeout=120, check=False)
        assert result.returncode == 0, result.stderr + result.stdout
        value: dict[str, Any] = json.loads(result.stdout)
        return value

    with tempfile.TemporaryDirectory(prefix="tf-sql-") as temporary:
        root = Path(temporary).resolve()
        launcher = root / "worker"
        launcher.write_text(
            f"#!{sys.executable} -I\nimport sys\nsys.path.insert(0, {str(wheel)!r})\n"
            "from transflow_worker.cli import main\n"
            "raise SystemExit(main(sys.argv[sys.argv.index('transflow_worker')+1:]))\n"
        )
        launcher.chmod(0o700)

        def case(
            name: str,
            sql: str,
            *,
            ok: bool = True,
            mutate: bool = False,
            params: str = "",
            table: Any = None,
            engine: str = "duckdb",
            equivalent: bool = False,
            storage_reject: bool = False,
        ) -> dict[str, Any]:
            directory = root / name
            directory.mkdir()
            workspace, source, capture, output = (
                directory / n for n in ["workspace", "source", "capture", "output"]
            )
            source.mkdir()
            output.mkdir(mode=0o700)
            (capture / "src").mkdir(parents=True)
            table = table if table is not None else pa.table({"id": [0, 199], "label": ["a", "b"]})
            pq.write_table(table, source / "exact.parquet")
            pq.write_table(pa.table({"id": [999], "label": ["hidden"]}), source / "hidden.parquet")
            if storage_reject:
                refused = subprocess.run(
                    [str(probe), "stage", str(workspace), str(source), "exact.parquet"],
                    capture_output=True,
                    text=True,
                    timeout=120,
                    check=False,
                )
                assert refused.returncode != 0 and "Unsupported Arrow type" in refused.stderr
                checks.append(name)
                return {"storage_rejected": True}
            binding = command([str(probe), "stage", str(workspace), str(source), "exact.parquet"])
            text = (
                "from transflow import sql_transform, Input, Output, Parameter\n"
                "produce = sql_transform('produce.sql', rows=Input('raw/items'), "
                f"output=Output('curated/items'){params})\n"
            )
            if engine == "polars":
                text = (
                    "from transflow import transform, Input, Output\n"
                    "@transform(rows=Input('raw/items'),output=Output('curated/items'))\n"
                    "def produce(rows): return rows\n"
                )
            py = capture / "src/produce.py"
            py.write_text(text)
            sql_path = capture / "src/produce.sql"
            sql_path.write_text(sql)
            files = [
                {
                    "path": str(p.relative_to(capture)),
                    "sha256": hashlib.sha256(p.read_bytes()).hexdigest(),
                    "byte_length": str(p.stat().st_size),
                }
                for p in [py, sql_path]
            ]
            namespace: dict[str, Any] = {"__name__": "produce", "__file__": str(py)}
            exec(compile(text, str(py), "exec"), namespace)
            declaration = get_declaration(namespace["produce"])
            assert declaration is not None
            entry = ModuleIndex.build(("src",), ("src/produce.py", "src/produce.sql")).modules[0]
            producer = json.loads(json.dumps(_declaration(declaration, entry)))
            producer["function"] = "produce"
            if engine == "duckdb":
                producer["sql_source"] = files[1]
            catalog = {
                "format_version": 1,
                "workspace_id": str(uuid4()),
                "source_snapshot_id": str(uuid4()),
                "catalog_fingerprint": "0" * 64,
                "entries": [],
            }
            catalog["catalog_fingerprint"] = catalog_fingerprint(catalog).hex
            request = {
                "format_version": 1,
                "protocol": {"major": 1, "minor": 0},
                "request_id": str(uuid4()),
                "attempt_id": str(uuid4()),
                "auth_token": "0" * 64,
                "capture_root": str(capture),
                "source_roots": ["src"],
                "files": files,
                "catalog": catalog,
                "environment_fingerprint": "a" * 64,
                "result_directory": str(output),
                "producer": producer,
                "inputs": [
                    {
                        "alias": "rows",
                        "dataset": {
                            "workspace_id": catalog["workspace_id"],
                            "dataset_id": str(uuid4()),
                        },
                        "version_id": str(uuid4()),
                        **binding,
                    }
                ],
                "compression": "zstd",
                "row_group_size": "8192",
                "threads": "2",
                "memory_bytes": None,
                "spill_bytes": "268435456",
                "context": {
                    "build_id": str(uuid4()),
                    "job_id": str(uuid4()),
                    "evaluation_time": {
                        "type": "timestamp",
                        "value": "2026-10-13T00:00:00.000000Z",
                        "unit": "us",
                        "timezone": "UTC",
                    },
                    "random_seed": "42",
                    "parameters": [
                        {"name": p["name"], "value": p["default"]} for p in producer["parameters"]
                    ],
                },
            }
            if engine == "polars":
                request.pop("memory_bytes")
                request.pop("spill_bytes")
            if mutate:
                sql_path.write_text(sql + " -- changed")
            req = directory / "request.json"
            req.write_text(json.dumps(request))
            result = command([str(probe), "execute", str(workspace), str(launcher), str(req)])
            assert result["execution"]["ok"] == ok, (name, result)
            assert result["head_generation"] == result["versions"] == result["events"] == 1
            checks.append(name)
            if ok:
                actual = command(
                    [str(normalization), "inspect", str(output / "part-00000.parquet")]
                )
                if equivalent:
                    expected = command(
                        [str(normalization), "inspect", str(source / "exact.parquet")]
                    )
                    assert actual == expected, (name, actual, expected)
                    if check_probe is not None:
                        subject = command(
                            [str(probe), "stage", str(workspace), str(output), "part-00000.parquet"]
                        )
                        declarations = [
                            Check(E.schema().equals(expected["schema"]), "Schema"),
                            Check(E.row_count().equals(table.num_rows), "Row count"),
                        ]
                        for column in table.column_names:
                            declarations.append(
                                Check(
                                    E.col(column)
                                    .null_count()
                                    .equals(table.column(column).null_count),
                                    "Null count " + column,
                                )
                            )
                        if "float64" in table.column_names:
                            declarations.append(
                                Check(E.col("float64").is_finite(), "Finite values")
                            )
                        result_directory = directory / "checks"
                        result_directory.mkdir(mode=0o700)
                        check_request = {
                            "format_version": 1,
                            "protocol": {"major": 1, "minor": 0},
                            "request_id": str(uuid4()),
                            "attempt_id": str(uuid4()),
                            "auth_token": "0" * 64,
                            "result_directory": str(result_directory),
                            **subject,
                            "phase": "output",
                            "subject_version": None,
                            "consumer_definition": "a" * 64,
                            "binding": None,
                            "threads": "1",
                            "memory_bytes": None,
                            "spill_bytes": "268435456",
                            "checks": [_check(c.effective()) for c in declarations],
                            "queries": [],
                        }
                        path = directory / "checks.json"
                        path.write_text(json.dumps(check_request))
                        evaluated = command(
                            [check_probe, str(workspace), str(launcher), str(path), "output"]
                        )["result"]["checks"]
                        assert [c["status"] for c in evaluated] == ["PASS"] * (
                            len(declarations) - (1 if "float64" in table.column_names else 0)
                        ) + (["VIOLATION"] if "float64" in table.column_names else [])
                        comparison = [
                            {k: c[k] for k in ["id", "status", "exact", "failed_rows", "metrics"]}
                            for c in evaluated
                        ]
                        group = name.rsplit("-", 1)[0]
                        if group in matrix_evaluations:
                            assert matrix_evaluations[group] == comparison
                        else:
                            matrix_evaluations[group] = comparison
                return actual
            return result

        exact = case("exact-alias", 'SELECT * FROM "rows" ORDER BY id')
        assert [r[0]["value"] for r in exact["rows"]] == ["0", "199"]
        filtered = case(
            "bound-parameter",
            'SELECT * FROM "rows" WHERE id > $minimum',
            params=", params={'minimum': Parameter({'type':'i64'}, {'type':'i64','value':'100'})}",
        )
        assert len(filtered["rows"]) == 1
        injection = case(
            "bound-injection",
            'SELECT * FROM "rows" WHERE label = $label',
            params=(
                ", params={'label': Parameter({'type':'string'}, "
                "{'type':'string','value':\"x' OR 1=1 --\"})}"
            ),
        )
        assert injection["rows"] == []
        case("empty-result", 'SELECT * FROM "rows" WHERE false')
        case("source-integrity", 'SELECT * FROM "rows"', ok=False, mutate=True)
        case("one-query", 'SELECT * FROM "rows"; SELECT 1', ok=False)
        case("no-extension-install", "INSTALL httpfs", ok=False)
        case("no-ambient-read", "SELECT * FROM read_parquet('/tmp/undeclared.parquet')", ok=False)
        case(
            "precision",
            'SELECT * FROM "rows"',
            table=pa.table(
                {
                    "value": pa.array([2**64 - 1], type=pa.uint64()),
                    "stamp": pa.array([1730000000123456789], type=pa.timestamp("ns")),
                }
            ),
        )
        case(
            "timezone-rejected",
            'SELECT * FROM "rows"',
            table=pa.table(
                {"value": pa.array([1730000000123456789], type=pa.timestamp("ns", tz="UTC"))}
            ),
            ok=False,
        )
        scalar_values: dict[str, Any] = {
            "flag": pa.array([True, False, None], type=pa.bool_()),
            "label": pa.array(["alpha", "beta", None], type=pa.string()),
            "bytes": pa.array([b"a", b"\x00", None], type=pa.binary()),
            "date": pa.array([0, 20000, None], type=pa.date32()),
            "float32": pa.array([-0.0, float("nan"), float("inf")], type=pa.float32()),
            "float64": pa.array([-0.0, float("nan"), -float("inf")], type=pa.float64()),
        }
        for width in [8, 16, 32, 64]:
            scalar_values[f"i{width}"] = pa.array(
                [-(2 ** (width - 1)), 2 ** (width - 1) - 1, None], type=getattr(pa, f"int{width}")()
            )
            scalar_values[f"u{width}"] = pa.array(
                [0, 2**width - 1, None], type=getattr(pa, f"uint{width}")()
            )
        precision_values = pa.table(
            {
                "decimal18": pa.array(
                    [Decimal("1234567890123456.78"), Decimal("-0.01"), None],
                    type=pa.decimal128(18, 2),
                ),
                "decimal38": pa.array(
                    [Decimal("999999999999999999999999999999999999.99"), Decimal("-0.01"), None],
                    type=pa.decimal128(38, 2),
                ),
                "ns": pa.array([1730000000123456789, -1, None], type=pa.timestamp("ns")),
                "us": pa.array([1730000000123456, -1, None], type=pa.timestamp("us")),
                "ms": pa.array([1730000000123, -1, None], type=pa.timestamp("ms")),
                "utc_us": pa.array([1730000000123456, -1, None], type=pa.timestamp("us", tz="UTC")),
            }
        )
        nested_values = pa.table(
            {
                "list": pa.array([[1, None], [], None], type=pa.list_(pa.int64())),
                "struct": pa.array(
                    [{"code": 2**64 - 1, "label": "a"}, {"code": 0, "label": None}, None],
                    type=pa.struct([("code", pa.uint64()), ("label", pa.string())]),
                ),
            }
        )
        for label, table in [
            ("scalars", pa.table(scalar_values)),
            ("precision", precision_values),
            ("nested", nested_values),
        ]:
            for engine in ["polars", "duckdb"]:
                case(
                    f"matrix-{label}-{engine}",
                    'SELECT * FROM "rows"',
                    table=table,
                    engine=engine,
                    equivalent=True,
                )
        case(
            "utc-millisecond-sql-conversion-rejected",
            'SELECT * FROM "rows"',
            table=pa.table({"stamp": pa.array([1730000000123], type=pa.timestamp("ms", tz="UTC"))}),
            ok=False,
        )
        case(
            "non-utc-timezone-rejected",
            'SELECT * FROM "rows"',
            table=pa.table(
                {"stamp": pa.array([1730000000123456], type=pa.timestamp("us", tz="Asia/Dubai"))}
            ),
            ok=False,
        )
        case(
            "decimal256-rejected",
            'SELECT * FROM "rows"',
            table=pa.table({"amount": pa.array([Decimal("1.00")], type=pa.decimal256(40, 2))}),
            ok=False,
            storage_reject=True,
        )
    print(
        json.dumps(
            {
                "format_version": 1,
                "checks": checks,
                "engine": "duckdb",
                "version": importlib.import_module("duckdb").__version__,
                "canonical_matrix_groups": sorted(matrix_evaluations),
            }
        )
    )


if __name__ == "__main__":
    main()
