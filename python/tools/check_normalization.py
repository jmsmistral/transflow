"""Qualify production Rust normalization against the pinned Arrow/Polars/DuckDB engines."""

import importlib
import json
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any


def main() -> None:
    executable = str(Path(sys.argv[1]).resolve(strict=True))
    pa = importlib.import_module("pyarrow")
    pq = importlib.import_module("pyarrow.parquet")
    pl = importlib.import_module("polars")
    duckdb = importlib.import_module("duckdb")

    def report(mode: str, path: Path, *, succeeds: bool = True) -> dict[str, Any]:
        result = subprocess.run(
            [executable, mode, str(path)], capture_output=True, text=True, timeout=60, check=False
        )
        if not succeeds:
            assert result.returncode != 0, "Unsupported normalization must fail"
            return {}
        assert result.returncode == 0, result.stderr
        value: dict[str, Any] = json.loads(result.stdout)
        return value

    with tempfile.TemporaryDirectory(prefix="transflow-normalization-") as directory:
        root = Path(directory)
        expected = report("generate", root / "source")
        source = root / "source/precision.parquet"
        table = pq.read_table(source)
        # Exact values travel through Arrow arrays, never Python datetime or float casts.
        pq.write_table(table, root / "arrow.parquet")
        assert report("inspect", root / "arrow.parquet") == expected
        pl.read_parquet(source).write_parquet(root / "polars.parquet")
        assert report("inspect", root / "polars.parquet") == expected
        pl.read_parquet(source).head(0).write_parquet(root / "empty.parquet")
        empty = report("inspect", root / "empty.parquet")
        assert empty["schema"] == expected["schema"] and empty["rows"] == []
        # The capability guard excludes aware nanoseconds from DuckDB exact validation.
        selected = [i for i, field in enumerate(table.schema) if field.name != "aware_ns"]
        compatible = table.select(selected)
        with duckdb.connect(
            config={"autoinstall_known_extensions": False, "autoload_known_extensions": False}
        ) as connection:
            connection.execute("SET TimeZone = 'UTC'")
            relation = connection.from_arrow(compatible)
            pq.write_table(relation.to_arrow_table(), root / "duckdb.parquet")
        actual = report("inspect", root / "duckdb.parquet")
        assert actual["schema"]["fields"] == [expected["schema"]["fields"][i] for i in selected]
        assert actual["rows"] == [[row[i] for i in selected] for row in expected["rows"]]
        # Native objects never become strings or opaque binary through this boundary.
        try:
            pl.DataFrame({"object": pl.Series([object()], dtype=pl.Object)}).write_parquet(
                root / "object.parquet"
            )
        except pl.exceptions.ComputeError:
            pass
        else:
            raise AssertionError("Object column was silently coerced")
        pq.write_table(
            pa.table({"duration": pa.array([1], type=pa.duration("ns"))}), root / "bad.parquet"
        )
        report("inspect", root / "bad.parquet", succeeds=False)
    print(
        json.dumps(
            {
                "result": "pass",
                "checks": 7,
                "engines": {
                    "pyarrow": pa.__version__,
                    "polars": pl.__version__,
                    "duckdb": duckdb.__version__,
                },
            }
        )
    )


if __name__ == "__main__":
    main()
