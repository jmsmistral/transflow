"""Run native engine compatibility checks; never import a Transflow package."""

from __future__ import annotations

import argparse
from decimal import Decimal
import importlib.metadata
import json
import math
from pathlib import Path
import platform
import re
import sqlite3
import sys
import tempfile

import duckdb
import pandas as pd
import polars as pl
import pyarrow as pa
import pyarrow.parquet as pq


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def probe(rust_parquet: Path | None) -> dict[str, object]:
    expected_versions = {
        "polars": "1.44.2", "duckdb": "1.5.5", "pyarrow": "25.0.1", "pandas": "3.0.6"
    }
    versions = {name: importlib.metadata.version(name) for name in expected_versions}
    require(versions == expected_versions, f"Unqualified engine versions: {versions}")
    require(sys.version_info[:2] in {(3, 13), (3, 14)}, "Only Python 3.13/3.14 are in this matrix")
    lock_path = Path(__file__).with_name(f"py{sys.version_info.major}{sys.version_info.minor}.lock")
    locked = dict(re.findall(r"^([A-Za-z0-9_-]+)==([^\s\\]+)", lock_path.read_text(), re.M))
    require(bool(locked), "Engine lock has no package pins")
    for name, version in locked.items():
        require(importlib.metadata.version(name) == version, f"Installed {name} differs from its lock")
    checks = []
    with tempfile.TemporaryDirectory(prefix="transflow-engine-probe-") as directory:
        root = Path(directory)
        table = pa.table({
            "id": pa.array([2**53 + 1, None, -(2**63)], type=pa.int64()),
            "label": pa.array(["alpha", None, "gamma"], type=pa.string()),
            "amount": pa.array([Decimal("12345678901234567890.1234"), None, Decimal("-0.0001")], type=pa.decimal128(38, 4)),
            "event_time": pa.array([1710000000000000001, None, 1710000000000000003], type=pa.timestamp("ns")),
            "reading": pa.array([float("nan"), None, 2.5], type=pa.float64()),
        })
        path = root / "polars.parquet"
        pl.from_arrow(table).lazy().sink_parquet(path, compression="zstd")
        reread = pq.read_table(path)
        for name in ("id", "label", "amount", "event_time"):
            require(reread[name].cast(table[name].type).equals(table[name]), f"Polars/Parquet changed {name}")
        readings = reread["reading"].to_pylist()
        require(math.isnan(readings[0]) and readings[1] is None and readings[2] == 2.5, "Null and NaN collapsed")
        require(reread["event_time"].type == pa.timestamp("ns"), "Timestamp unit was lost")
        checks.append("polars_streaming_sink_pyarrow_roundtrip")

        with duckdb.connect() as db:
            row = db.execute(
                "SELECT id, amount, epoch_ns(event_time), isnan(reading) FROM read_parquet(?) WHERE id > 0", [str(path)]
            ).fetchone()
            require(row == (2**53 + 1, Decimal("12345678901234567890.1234"), 1710000000000000001, True), "DuckDB lost numeric or timestamp precision")
            nulls = db.execute("SELECT count(*) FROM read_parquet(?) WHERE id IS NULL AND reading IS NULL", [str(path)]).fetchone()
            require(nulls == (1,), "DuckDB did not preserve nulls")
            duck_version = db.execute("SELECT version()").fetchone()[0]
            memory_limit = db.execute("SELECT current_setting('memory_limit')").fetchone()[0]
            checks.append("duckdb_exact_parquet_values")
            try:
                db.execute("SELECT missing_column FROM read_parquet(?)", [str(path)])
            except duckdb.BinderException:
                checks.append("duckdb_missing_column_rejected")
            else:
                raise RuntimeError("DuckDB accepted an unknown column")

        frame = table.to_pandas(types_mapper=pd.ArrowDtype)
        pandas_path = root / "pandas.parquet"
        frame.to_parquet(pandas_path, engine="pyarrow", index=False)
        pandas_back = pq.read_table(pandas_path)
        for name in ("id", "label", "amount", "event_time"):
            require(pandas_back[name].equals(table[name]), f"Arrow-backed pandas changed {name}")
        require(pandas_back.column_names == table.column_names, "pandas added an implicit index")
        pandas_readings = pandas_back["reading"].to_pylist()
        require(math.isnan(pandas_readings[0]) and pandas_readings[1] is None, "pandas collapsed null and NaN")
        checks.append("pandas_arrow_backed_parquet_roundtrip")

        empty = root / "empty.parquet"
        pl.from_arrow(table.slice(0, 0)).lazy().sink_parquet(empty)
        require(pq.read_metadata(empty).num_rows == 0, "Empty Parquet has rows")
        require(pq.read_schema(empty).field("event_time").type == pa.timestamp("ns"), "Empty Parquet lost its schema")
        checks.append("empty_parquet_schema")

    if rust_parquet is not None:
        rust_table = pq.read_table(rust_parquet)
        require(rust_table["id"].to_pylist() == [9007199254740993, 2, None], "Rust-to-Python integers changed")
        require(rust_table["label"].to_pylist() == ["alpha", None, "gamma"], "Rust-to-Python strings/nulls changed")
        checks.append("rust_to_python_parquet")
    return {
        "python": platform.python_version(), "platform": platform.platform(),
        "machine": platform.machine(), "packages": versions, "locked_packages": locked, "duckdb_runtime": duck_version,
        "duckdb_native_memory_limit": memory_limit,
        "python_sqlite": {"version": sqlite3.sqlite_version, "role": "informational; not the Rust control plane"},
        "checks": checks,
        "limits": ["Not full logical-type conformance", "No DuckDB query-security qualification (T009)", "No matched SDK/worker packages yet (T005)"]
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rust-parquet", type=Path)
    args = parser.parse_args()
    print(json.dumps(probe(args.rust_parquet), indent=2))
