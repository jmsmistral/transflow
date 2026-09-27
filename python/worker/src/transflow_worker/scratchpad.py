"""Private one-query process entry point. No producer imports or publication authority.

Run with ``python -I -B -m transflow_worker.scratchpad REQUEST``. The application
must supply exact leased bindings and supervise the process through termination;
T080 owns that public lifecycle. This helper never resolves a moving dataset head.
"""

from __future__ import annotations

import importlib
import json
import os
import sys
import tempfile
from pathlib import Path
from typing import Any

from .check_adapter import pinned_files
from .discovery import _directory, _read
from .environment import MANAGED_PACKAGES
from .polars_adapter import _file
from .sql_policy import SqlPolicyError, require, validate_query, view_names
from .wire import _decode, validate_document

ROWS = 1000
BYTES = 2 * 1024 * 1024
# This optional native dependency is pinned by the native qualification lock.
# T080 must check availability before advertising the public scratchpad service.
ARROW_VERSION = "25.0.1"


def _schema(schema: Any, pa: Any) -> None:
    """Refuse known lossy DuckDB inputs before binding; Arrow retains exact result types."""
    require(len(schema) <= 128, "SQL inputs support at most 128 columns")
    names = [field.name.lower() for field in schema]
    require(len(set(names)) == len(names), "Case-colliding input columns are unsupported")
    for field in schema:
        kind = field.type
        if pa.types.is_decimal(kind):
            require(
                kind.precision <= 38 and 0 <= kind.scale <= kind.precision,
                "SQL input decimal precision or scale is unsupported",
            )
        elif pa.types.is_timestamp(kind):
            require(
                kind.unit in {"ms", "us", "ns"}
                and (kind.tz is None or kind.tz == "UTC" and kind.unit != "ns"),
                "SQL input timestamp precision or timezone is unsupported",
            )
        else:
            require(
                pa.types.is_boolean(kind)
                or pa.types.is_integer(kind)
                or pa.types.is_floating(kind)
                or pa.types.is_string(kind)
                or pa.types.is_large_string(kind)
                or pa.types.is_binary(kind)
                or pa.types.is_large_binary(kind)
                or pa.types.is_date32(kind),
                "SQL input type is outside the qualified scalar subset",
            )


def _connect(
    paths: dict[str, list[str]], temporary: Path, limits: dict[str, Any] | None = None
) -> Any:
    limits = limits or {"threads": 1, "spill_bytes": "268435456", "memory_bytes": None}
    duckdb = importlib.import_module("duckdb")
    require(duckdb.__version__ == MANAGED_PACKAGES["duckdb"], "Unqualified SQL engine version")
    db = duckdb.connect(
        config={
            "autoinstall_known_extensions": False,
            "autoload_known_extensions": False,
            "allow_community_extensions": False,
            "allow_unsigned_extensions": False,
            "allow_persistent_secrets": False,
            "python_enable_replacements": False,
            "threads": limits["threads"],
        }
    )
    try:
        settings = {
            "allowed_paths": sorted({path for files in paths.values() for path in files}),
            "allowed_directories": [],
            "allowed_configs": [],
            "temp_directory": str(temporary),
            "max_temp_directory_size": f"{limits['spill_bytes']}B",
            "extension_directory": str(temporary / "extensions"),
            "secret_directory": str(temporary / "secrets"),
            "enable_progress_bar": False,
            "TimeZone": "UTC",
        }
        for name, value in settings.items():
            db.execute(f"SET {name} = ?", [value])  # Application constants, never authored keys.
        if limits["memory_bytes"] is not None:
            db.execute("SET memory_limit = ?", [f"{limits['memory_bytes']}B"])
        db.execute("SET enable_external_access = false")
        for alias, files in paths.items():
            db.read_parquet(files, hive_partitioning=False, union_by_name=False).create_view(alias)
        db.execute("SET lock_configuration = true")
        return db
    except BaseException:
        db.close()
        raise


def _results(
    db: Any, request: dict[str, Any], output: Path, pa: Any, limits: dict[str, Any] | None = None
) -> dict[str, Any]:
    max_rows = limits["rows"] if limits else ROWS
    max_bytes = limits["bytes"] if limits else BYTES
    # No string interpolation, relation.sql() or Python replacement scans. The exact
    # validated statement reaches the same pinned connection, with parameters separate.
    reader = db.execute(request["sql"], request["parameters"]).to_arrow_reader(batch_size=128)
    require(len(reader.schema) <= 128, "SQL results support at most 128 columns")
    batches: list[Any] = []
    rows, truncated = 0, False

    def encode() -> Any:
        sink = pa.BufferOutputStream()
        with pa.ipc.new_stream(sink, reader.schema) as writer:
            for batch in batches:
                writer.write_batch(batch)
        return sink.getvalue()

    payload = encode()
    require(payload.size <= max_bytes, "SQL result schema exceeds the byte limit")
    with reader:
        for batch in reader:
            if rows == max_rows:
                truncated = True
                break
            # Bound retained output before serialization, including a single oversized
            # cell. Decode/query working memory is not promised to be hard-bounded here.
            take = min(batch.num_rows, max_rows - rows)
            candidate = batch.slice(0, take)
            if candidate.nbytes > max_bytes:
                truncated = True
                break
            batches.append(candidate)
            encoded = encode()
            if encoded.size > max_bytes:
                batches.pop()
                truncated = True
                break
            payload, rows = encoded, rows + take
            if take < batch.num_rows:
                truncated = True
                break
    with output.open("xb") as stream:
        os.chmod(output, 0o600)
        stream.write(payload)
    return {
        "format_version": 1,
        "request_id": request["request_id"],
        "row_count": str(rows),
        "truncated": truncated,
        "bindings": [
            {
                k: binding[k]
                for k in ("alias", "workspace_id", "dataset_id", "version_id", "artifact_digest")
            }
            for binding in request["bindings"]
        ],
    }


def execute(request: dict[str, Any], limits: dict[str, Any] | None = None) -> None:
    """Application-only entry; caller owns read leases and process/resource supervision."""
    validate_document("ScratchpadRequestV1", request)
    if limits is not None:
        validate_document("QueryLimitsV1", limits)
        require(int(limits["spill_bytes"]) <= 268435456, "Query spill exceeds the service ceiling")
    view_names([binding["alias"] for binding in request["bindings"]])
    # Validate text size before importing engines or reading any artifact bytes.
    require(len(request["sql"].encode()) <= 16384, "SQL exceeds the text limit")
    directory = _directory(request["result_directory"], private=True)
    require(not any(directory.iterdir()), "SQL results require an empty private directory")
    pa = importlib.import_module("pyarrow")
    require(pa.__version__ == ARROW_VERSION, "Unqualified Arrow version")
    pq = importlib.import_module("pyarrow.parquet")
    paths: dict[str, list[str]] = {}
    guards: list[tuple[str, tuple[int, int, int, int, int]]] = []
    for binding in request["bindings"]:
        files, identities = pinned_files(binding)
        require(not any(any(c in p for c in "*?[") for p in files), "SQL input globs are forbidden")
        expected = None
        for path in files:
            schema = pq.read_schema(path)
            _schema(schema, pa)
            require(expected is None or schema.equals(expected), "SQL input file schemas differ")
            expected = schema
        paths[binding["alias"]] = files
        guards.extend(zip(files, identities, strict=True))
    output = directory / "result.arrow"
    metadata = directory / "result.json"
    try:
        # DuckDB adds this one owned empty directory to allowed_directories. No
        # provider or workspace directory is granted. Temporary files never publish.
        with tempfile.TemporaryDirectory(prefix="spill-", dir=directory) as temporary:
            with _connect(paths, Path(temporary), limits) as db:
                validate_query(db, request["sql"], list(paths))
                result = _results(db, request, output, pa, limits)
        require(
            all(_file(Path(path)) == guard for path, guard in guards),
            "SQL inputs changed during the query",
        )
        validate_document("ScratchpadResultV1", result)
        with metadata.open("x", encoding="utf-8") as stream:
            os.chmod(metadata, 0o600)
            json.dump(result, stream, allow_nan=False)
    except BaseException:
        output.unlink(missing_ok=True)
        metadata.unlink(missing_ok=True)
        raise


def main() -> int:
    """No stdout control/rows and no arbitrary exception details on the private boundary."""
    try:
        require(
            sys.flags.isolated == 1 and len(sys.argv) == 2,
            "SQL helper requires isolated invocation and one private request",
        )
        # The launcher must also supply a clean environment. Clear inherited settings
        # before loading engines so source credentials/configuration cannot enter them.
        os.environ.clear()
        os.umask(0o077)
        request_path = Path(sys.argv[1])
        _directory(str(request_path.parent), private=True)
        request = _decode(_read(request_path, 1024 * 1024, private=True))
        execute(request)
        return 0
    except SqlPolicyError as error:
        print(str(error), file=sys.stderr)
        return 1
    except Exception:
        print(
            "SQL helper failed; inspect bindings, supported types and query syntax", file=sys.stderr
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
