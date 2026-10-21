"""One captured SQL producer, exact input files, one sink, no publication authority."""

from __future__ import annotations

import hashlib
import importlib
import os
import sys
from collections.abc import Callable
from datetime import datetime
from pathlib import Path
from types import MappingProxyType
from typing import Any

from transflow import Parameter
from transflow.declarations import SqlTransform, get_declaration

from .arrow_schema import logical_schema, quote, sql_type
from .canonical import file_digest, schema_digest
from .check_adapter import pinned_files
from .dataset_handles import arguments
from .discovery import _directory, _read, _write, collect
from .environment import ARROW_VERSION
from .polars_adapter import Context, ExecutionError, _file, _value
from .python_preview import execution_declaration
from .registration import resolve
from .scratchpad import _connect
from .wire import validate_document


def execute(request: dict[str, Any], send: Callable[[dict[str, object]], None]) -> dict[str, Any]:
    validate_document("SqlExecutionRequestV1", request)
    directory = _directory(request["result_directory"], private=True)
    threads, row_group = int(request["threads"]), int(request["row_group_size"])
    if not 1 <= threads <= 65535 or not 2048 <= row_group <= 1048576:
        raise ExecutionError(
            "resource", "SQL threads or row-group size is outside supported bounds"
        )
    producer = request["producer"]
    aliases = [value["alias"] for value in producer["inputs"]]
    if len(set(aliases)) != len(aliases) or aliases != [v["alias"] for v in request["inputs"]]:
        raise ExecutionError("bindings", "Pinned inputs must exactly match the ordered SQL aliases")
    fields = {
        "format_version",
        "protocol",
        "request_id",
        "attempt_id",
        "capture_root",
        "source_roots",
        "files",
        "catalog",
        "environment_fingerprint",
        "result_directory",
        "auth_token",
    }
    discovered = collect({key: request[key] for key in fields})
    definitions = discovered["definitions"]
    if not isinstance(definitions, list) or execution_declaration(producer) not in [
        execution_declaration(item) for item in definitions
    ]:
        raise ExecutionError(
            "definition_changed", "Captured SQL producer differs from the accepted declaration"
        )
    registration = resolve(sys.modules[producer["module"]], producer)
    declaration = get_declaration(registration)
    if declaration is None or not (
        isinstance(registration, SqlTransform) or callable(registration)
    ):
        raise ExecutionError("definition", "SQL execution requires its captured registration")
    supplied = {v["name"]: v["value"] for v in request["context"]["parameters"]}
    if len(supplied) != len(request["context"]["parameters"]) or set(supplied) != {
        n for n, _ in declaration.params
    }:
        raise ExecutionError("parameter", "SQL parameters must match the accepted declaration")
    parameters = {}
    for name, parameter in declaration.params:
        Parameter(parameter.logical_type, supplied[name])
        parameters[name] = _value(supplied[name])
    send({"type": "phase", "phase": "running"})
    if isinstance(registration, SqlTransform):
        source = producer["sql_source"]
        raw = _read(Path(request["capture_root"]) / source["path"], 16 * 1024 * 1024)
        if (
            len(raw) != int(source["byte_length"])
            or hashlib.sha256(raw).hexdigest() != source["sha256"]
        ):
            raise ExecutionError("sql_source", "Captured SQL source bytes changed")
        sql = raw.decode("utf-8")
    else:
        timestamp = _value(request["context"]["evaluation_time"])
        if not isinstance(timestamp, datetime) or timestamp.tzinfo is None:
            raise ExecutionError("context", "Logical evaluation time must be an aware timestamp")
        seed = request["context"]["random_seed"]
        context = Context(
            MappingProxyType(parameters),
            request["context"]["build_id"],
            request["context"]["job_id"],
            request["attempt_id"],
            timestamp,
            None if seed is None else int(seed),
        )
        sql = registration(**arguments(registration, declaration, request, {}, context))
    if (
        not isinstance(sql, str)
        or not sql.strip()
        or (not isinstance(registration, SqlTransform) and len(sql.encode("utf-8")) > 512 * 1024)
    ):
        raise ExecutionError(
            "sql_return", "SQL factory must return one nonempty SQL string of at most 512 KiB"
        )
    # Durable attempt evidence is written before parsing/execution, including failed queries.
    # Bound values remain separate; the SQL is never expanded with parameter values.
    if len(sql.encode("utf-8")) <= 512 * 1024:
        _write(
            directory / "generated-sql.json",
            {
                "format_version": 1,
                "attempt": request["attempt_id"],
                "source": request["catalog"]["source_snapshot_id"],
                "producer": producer,
                "inputs": [
                    {"alias": b["alias"], "dataset": b["dataset"], "version": b["version_id"]}
                    for b in request["inputs"]
                ],
                "sha256": hashlib.sha256(sql.encode()).hexdigest(),
                "sql": sql,
            },
        )
    pa, pq = importlib.import_module("pyarrow"), importlib.import_module("pyarrow.parquet")
    if pa.__version__ != ARROW_VERSION:
        raise ExecutionError("engine_version", "SQL requires the qualified Arrow version")
    paths, guards = {}, []
    projections = {}
    schemas = {}
    for binding, declared in zip(request["inputs"], producer["inputs"], strict=True):
        files, identities = pinned_files(binding)
        if any(any(c in path for c in "*?[") for path in files):
            raise ExecutionError(
                "input_path", "SQL pinned filenames cannot contain glob characters"
            )
        for path, identity in zip(files, identities, strict=True):
            if logical_schema(pq.read_schema(path), pa) != binding["manifest"]["logical_schema"]:
                raise ExecutionError(
                    "input_schema", "SQL input schema differs from its pinned manifest"
                )
            guards.append((Path(path), identity))
        if declared["role"] == "data":
            paths[binding["alias"]] = files
            schema = binding["manifest"]["logical_schema"]
            schemas[binding["alias"]] = schema
            projections[binding["alias"]] = ",".join(
                f"CAST({quote(f['name'])} AS {sql_type(f['logical_type'])}) AS {quote(f['name'])}"
                for f in schema["fields"]
            )
    temporary = directory / "spill"
    temporary.mkdir(mode=0o700)
    output = directory / "part-00000.parquet"
    if output.exists():
        raise ExecutionError("output_path", "SQL output must be a fresh private file")
    limits = {
        "threads": threads,
        "spill_bytes": request["spill_bytes"],
        "memory_bytes": request["memory_bytes"],
    }
    with _connect(paths, temporary, limits, output_path=str(output), projections=projections) as db:
        for alias, expected in schemas.items():
            if logical_schema(db.table(alias).limit(0).to_arrow_reader().schema, pa) != expected:
                raise ExecutionError(
                    "input_schema",
                    "DuckDB cannot preserve this input schema; normalize it explicitly before SQL",
                )
        statements = db.extract_statements(sql)
        if len(statements) != 1 or statements[0].type.name != "SELECT":
            raise ExecutionError(
                "sql_statement",
                "SQL producers require one table query without DDL or extension installation",
            )
        names = statements[0].named_parameters
        if not names <= parameters.keys():
            raise ExecutionError("parameter", "SQL references an undeclared bound parameter")
        bound = {name: parameters[name] for name in names}
        relation = db.sql(sql, params=bound or None)
        # Schema-only query carries no result rows. The authored relation is sunk once.
        schema = logical_schema(relation.limit(0).to_arrow_reader().schema, pa)
        if declaration.output.schema is not None and schema != declaration.output.schema:
            raise ExecutionError(
                "output_schema", "SQL output differs from the declared schema; cast explicitly"
            )
        send({"type": "phase", "phase": "materializing"})
        old_mask = os.umask(0o077)
        try:
            relation.write_parquet(
                str(output),
                compression=request["compression"],
                row_group_size=row_group,
                overwrite=False,
            )
        finally:
            os.umask(old_mask)
    with output.open("rb") as stream:
        os.fsync(stream.fileno())
        digest = file_digest(stream).hex
    actual = logical_schema(pq.read_schema(output), pa)
    if actual != schema or any(_file(path) != identity for path, identity in guards):
        raise ExecutionError(
            "artifact_integrity", "SQL schema or pinned input changed during materialization"
        )
    manifest = {
        "format_version": 1,
        "logical_schema": actual,
        "schema_fingerprint": schema_digest(actual).hex,
        "files": [
            {
                "path": output.name,
                "sha256": digest,
                "byte_length": str(output.stat().st_size),
                "row_count": str(pq.ParquetFile(output).metadata.num_rows),
            }
        ],
        "writer": {
            "engine": "duckdb",
            "version": importlib.import_module("duckdb").__version__,
            "compression": request["compression"],
            "row_group_size": request["row_group_size"],
        },
    }
    validate_document("ArtifactManifestV1", manifest)
    _write(directory / "artifact.json", manifest)
    return manifest
