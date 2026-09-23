"""Execute application-compiled aggregate queries in an isolated, restricted DuckDB connection.

This module never imports producer code, accepts authored SQL, or returns dataframe rows.
Rust owns the typed compiler and verifies the subject before and after this helper.
"""

from __future__ import annotations

import importlib
from pathlib import Path
from typing import Any

from .canonical import artifact_digest, file_digest
from .discovery import DiscoveryError, _directory
from .polars_adapter import _file


class CheckError(DiscoveryError):
    """Safe failure classification without engine text, bound values or row contents."""


def pinned_files(request: dict[str, Any]) -> tuple[list[str], list[tuple[int, int, int, int, int]]]:
    manifest = request["manifest"]
    if artifact_digest(manifest).hex != request["artifact_digest"]:
        raise CheckError("artifact_integrity", "Check subject digest does not match")
    root = _directory(request["artifact_root"])
    paths, guards = [], []
    for entry in manifest["files"]:
        path = root / entry["path"]
        guard = _file(path)
        with path.open("rb") as stream:
            digest = file_digest(stream).hex
        if (
            digest != entry["sha256"]
            or guard[2] != int(entry["byte_length"])
            or _file(path) != guard
        ):
            raise CheckError("artifact_integrity", "Check subject bytes changed")
        paths.append(str(path))
        guards.append(guard)
    return paths, guards


def connect(request: dict[str, Any], paths: list[str]) -> Any:
    duckdb = importlib.import_module("duckdb")
    if duckdb.__version__ != "1.5.5":
        raise CheckError("engine_version", "Canonical checks require the qualified DuckDB version")
    temporary = _directory(request["spill_directory"], private=True)
    if any(temporary.iterdir()):
        raise CheckError("spill_directory", "Canonical checks require fresh private spill storage")
    threads = int(request["threads"])
    if not 1 <= threads <= 65535:
        raise CheckError("threads", "Canonical checks require the admitted thread count")
    db = duckdb.connect(
        config={
            "autoinstall_known_extensions": False,
            "autoload_known_extensions": False,
            "allow_community_extensions": False,
            "allow_unsigned_extensions": False,
            "allow_persistent_secrets": False,
            "python_enable_replacements": False,
            "threads": threads,
        }
    )
    try:
        settings = {
            "allowed_paths": paths,
            "allowed_directories": [],
            "allowed_configs": [],
            "temp_directory": str(temporary),
            "max_temp_directory_size": f"{request['spill_bytes']}B",
            "extension_directory": str(temporary / "extensions"),
            "secret_directory": str(temporary / "secrets"),
            "enable_progress_bar": False,
            "TimeZone": "UTC",
        }
        for name, value in settings.items():
            db.execute(f"SET {name} = ?", [value])  # Fixed application setting names only.
        if request["memory_bytes"] is not None:
            db.execute("SET memory_limit = ?", [f"{request['memory_bytes']}B"])
        db.execute("SET enable_external_access = false")
        # Exact manifest paths: no glob, hive partition inference, union-by-name or SQL filenames.
        if any(any(c in p for c in "*?[") for p in paths):
            raise CheckError("artifact_path", "Canonical staged filenames must not contain globs")
        db.read_parquet(paths, hive_partitioning=False, union_by_name=False).create_view("subject")
        db.execute("SET lock_configuration = true")
        return db
    except BaseException:
        db.close()
        raise


def execute(request: dict[str, Any]) -> dict[str, Any]:
    paths, guards = pinned_files(request)
    rows: list[dict[str, Any]] = []
    samples: list[dict[str, Any] | None] = []
    with connect(request, paths) as db:
        for query in request["queries"]:
            # Only Rust's private typed compiler constructs these queries. Values remain
            # strings/bools and are explicitly cast by the compiler, preserving ns/decimal/u64.
            parameters = [v["value"] for v in query["parameters"]]
            try:
                cursor = db.execute(query["sql"], parameters)
                row = cursor.fetchone()
                if row is None or len(row) != query["width"] or cursor.fetchone() is not None:
                    raise CheckError(
                        "aggregate_shape", "Canonical query returned an invalid aggregate"
                    )
                counts = []
                for value in row:
                    count = int(value)
                    if value != count or count < 0 or count > 2**64 - 1:
                        raise CheckError(
                            "aggregate_range", "Canonical aggregate exceeds its exact range"
                        )
                    counts.append(str(count))
                rows.append({"counts": counts, "error": None})
            except Exception:
                # A query failure is ERROR, including under WARN. Continue independent queries.
                # Never retain engine messages: they can contain paths, literals and row values.
                rows.append({"counts": [], "error": "query_error"})
        from .check_samples import execute as sample

        for query in request.get("sample_queries", []):
            samples.append(None if query is None else sample(db, query))
    if any(_file(Path(path)) != guard for path, guard in zip(paths, guards, strict=True)):
        raise CheckError("artifact_integrity", "Check subject changed during evaluation")
    return {
        "format_version": 1,
        "request_id": request["request_id"],
        "artifact_digest": request["artifact_digest"],
        "queries": rows,
        **({"samples": samples} if "sample_queries" in request else {}),
    }
