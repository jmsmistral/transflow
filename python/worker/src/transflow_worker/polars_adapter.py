"""Polars data plane: exact manifest scans and one streaming sink, never publication."""

from __future__ import annotations

import importlib
import inspect
import os
import stat
import struct
import sys
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from datetime import date, datetime
from decimal import Decimal
from pathlib import Path
from types import MappingProxyType
from typing import Any
from zoneinfo import ZoneInfo

from transflow._declaration_values import DeclarationError, Parameter
from transflow.declarations import get_declaration, validate_result

from .canonical import artifact_digest, file_digest, schema_digest
from .discovery import DiscoveryError, _directory, _write, collect
from .wire import validate_document


class ExecutionError(DiscoveryError):
    """A safe execution failure, with optional captured source location."""


def logical_schema(pl: Any, schema: Mapping[str, Any]) -> dict[str, Any]:
    """Project Polars' supported Parquet output fields; Rust independently rechecks bytes."""
    remaining = 4096
    primitives = {
        pl.Boolean: "bool",
        pl.Int8: "i8",
        pl.Int16: "i16",
        pl.Int32: "i32",
        pl.Int64: "i64",
        pl.UInt8: "u8",
        pl.UInt16: "u16",
        pl.UInt32: "u32",
        pl.UInt64: "u64",
        pl.Float32: "f32",
        pl.Float64: "f64",
        pl.String: "string",
        pl.Binary: "binary",
        pl.Date: "date",
    }

    def field(name: str, dtype: Any, depth: int) -> dict[str, Any]:
        nonlocal remaining
        remaining -= 1
        if depth > 24 or remaining < 0:
            raise ExecutionError(
                "schema", "Output schema exceeds the supported depth or field limit"
            )
        logical: dict[str, Any]
        if dtype in primitives:
            logical = {"type": primitives[dtype]}
        elif isinstance(dtype, pl.Datetime):
            logical = {"type": "timestamp", "unit": dtype.time_unit, "timezone": dtype.time_zone}
        elif isinstance(dtype, pl.Decimal) and dtype.precision is not None:
            logical = {"type": "decimal", "precision": dtype.precision, "scale": dtype.scale}
        elif isinstance(dtype, pl.List):
            logical = {"type": "list", "element": field("element", dtype.inner, depth + 1)}
        elif isinstance(dtype, pl.Struct):
            logical = {
                "type": "struct",
                "fields": [field(f.name, f.dtype, depth + 1) for f in dtype.fields],
            }
        else:
            raise ExecutionError(
                "schema",
                "Polars output contains an unsupported logical type; explicitly convert it",
            )
        return {"name": name, "logical_type": logical, "nullable": True}

    result = {"format_version": 1, "fields": [field(n, t, 0) for n, t in schema.items()]}
    validate_document("LogicalSchemaV1", result)
    return result


def _types(schema: dict[str, Any]) -> object:
    """Input nullability/metadata are verified by Rust; Polars exposes only logical dtypes."""

    def clean(value: Any) -> Any:
        if isinstance(value, dict):
            return {k: clean(v) for k, v in value.items() if k not in {"nullable", "metadata"}}
        if isinstance(value, list):
            return [clean(v) for v in value]
        return value

    return clean(schema)


def _file(path: Path) -> tuple[int, int, int, int, int]:
    if not path.is_absolute() or path.resolve() != path:
        raise ExecutionError(
            "input_integrity", "Pinned file must be canonical and cannot traverse symlinks"
        )
    info = path.stat()
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
        raise ExecutionError(
            "input_integrity", "Pinned file must be a regular file without hard links"
        )
    return info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns


def _scan(pl: Any, binding: dict[str, Any]) -> Any:
    manifest = binding["manifest"]
    if artifact_digest(manifest).hex != binding["artifact_digest"]:
        raise ExecutionError("input_integrity", "Pinned artifact manifest digest does not match")
    root = _directory(binding["artifact_root"])
    paths: list[str] = []
    for entry in manifest["files"]:
        path = root / entry["path"]
        before = _file(path)
        with path.open("rb") as stream:
            digest = file_digest(stream).hex
        if (
            digest != entry["sha256"]
            or before[2] != int(entry["byte_length"])
            or _file(path) != before
        ):
            raise ExecutionError(
                "input_integrity", "Pinned file bytes changed; no branch fallback is permitted"
            )
        actual = logical_schema(pl, pl.read_parquet_schema(path))
        if _types(actual) != _types(manifest["logical_schema"]):
            raise ExecutionError("input_schema", "Pinned file schema does not match the manifest")
        paths.append(str(path))
    return pl.scan_parquet(
        paths,
        glob=False,
        hive_partitioning=False,
        cache=False,
        missing_columns="raise",
        extra_columns="raise",
    )


def _value(value: dict[str, Any]) -> object:
    kind = value["type"]
    if kind == "null":
        return None
    if kind in {"bool", "string"}:
        return value["value"]
    if kind in {"i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"}:
        return int(value["value"])
    if kind == "f32":
        return struct.unpack("!f", struct.pack("!f", float(value["value"])))[0]
    if kind == "f64":
        return float(value["value"])
    if kind == "decimal":
        return Decimal(value["value"])
    if kind == "date":
        return date.fromisoformat(value["value"])
    if kind == "timestamp":
        text = value["value"]
        if value["unit"] == "ns" and text.removesuffix("Z")[-3:] != "000":
            raise ExecutionError(
                "parameter", "Python datetime parameters cannot represent submicrosecond timestamps"
            )
        parsed = datetime.fromisoformat(text)
        return parsed.astimezone(ZoneInfo(value["timezone"])) if value["timezone"] else parsed
    if kind == "list":
        return tuple(_value(v) for v in value["values"])
    if kind == "struct":
        return MappingProxyType({f["name"]: _value(f["value"]) for f in value["fields"]})
    raise ExecutionError("parameter", "Unsupported execution parameter type")


@dataclass(frozen=True)
class Context:
    """Immutable execution context; secret resolution is a later source-service boundary."""

    params: Mapping[str, object]
    build_id: str
    job_id: str
    attempt_id: str
    evaluation_time: datetime
    random_seed: int | None

    @property
    def cancelled(self) -> bool:
        # Coordinator disconnect/TERM stops this owned process; there is no polling DB.
        return False

    def log(self, message: str) -> None:
        print(message, file=sys.stderr)

    def secret(self, reference: str) -> str:
        raise ExecutionError("secret", "Secret delivery requires the source execution service")


def execute(request: dict[str, Any], send: Callable[[dict[str, object]], None]) -> dict[str, Any]:
    """Run only in a fresh authenticated worker with pre-import resource environment."""
    validate_document("PolarsExecutionRequestV1", request)
    threads = int(request["threads"])
    row_group = int(request["row_group_size"])
    if not 1 <= threads <= 65535 or not 1 <= row_group <= 1048576:
        raise ExecutionError(
            "resource", "Polars threads or row-group size is outside supported bounds"
        )
    if "polars" in sys.modules or os.environ.get("POLARS_MAX_THREADS") != str(threads):
        raise ExecutionError(
            "resource", "Polars thread policy must be installed before engine imports"
        )
    try:
        pl = importlib.import_module("polars")
    except ImportError as exc:
        raise ExecutionError(
            "environment", "Polars is missing; prepare the managed environment explicitly"
        ) from exc
    if pl.thread_pool_size() != threads:
        raise ExecutionError(
            "resource", "Polars engine thread pool does not match the accepted policy"
        )
    directory = _directory(request["result_directory"], private=True)
    producer = request["producer"]
    aliases = [entry["alias"] for entry in producer["inputs"]]
    if len(aliases) != len(set(aliases)) or aliases != [
        entry["alias"] for entry in request["inputs"]
    ]:
        raise ExecutionError(
            "bindings", "Pinned inputs must exactly match the producer's ordered aliases"
        )
    frames = {entry["alias"]: _scan(pl, entry) for entry in request["inputs"]}
    # These are data-plane checks, not expectation evaluation. Runtime dispatch must
    # complete required input expectations before issuing this execute operation.
    discovery_fields = {
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
    discovered = collect({key: request[key] for key in discovery_fields})
    definitions = discovered["definitions"]
    if not isinstance(definitions, list) or producer not in definitions:
        raise ExecutionError(
            "definition_changed", "Captured producer no longer matches the accepted declaration"
        )
    function = getattr(sys.modules[producer["module"]], producer["function"], None)
    declaration = get_declaration(function)
    if declaration is None or not callable(function):
        raise ExecutionError(
            "definition", "Execution requires the exact captured decorated producer"
        )
    overrides = request["context"]["parameters"]
    if len({p["name"] for p in overrides}) != len(overrides):
        raise ExecutionError("parameter", "Duplicate execution parameters are not permitted")
    supplied = {p["name"]: p["value"] for p in overrides}
    if set(supplied) != {name for name, _ in declaration.params}:
        raise ExecutionError(
            "parameter", "Resolved execution parameters must match the accepted declaration"
        )
    params = {}
    for name, parameter in declaration.params:
        Parameter(parameter.logical_type, supplied[name])
        params[name] = _value(supplied[name])
    timestamp = _value(request["context"]["evaluation_time"])
    if not isinstance(timestamp, datetime) or timestamp.tzinfo is None:
        raise ExecutionError("context", "Logical evaluation time must be an aware timestamp")
    seed = request["context"]["random_seed"]
    context = Context(
        MappingProxyType(params),
        request["context"]["build_id"],
        request["context"]["job_id"],
        request["attempt_id"],
        timestamp,
        None if seed is None else int(seed),
    )
    arguments = {
        alias: frames[alias] for alias, binding in declaration.inputs if binding.role == "data"
    }
    if "ctx" in inspect.signature(function).parameters:
        arguments["ctx"] = context
    send({"type": "phase", "phase": "running"})
    result = function(**arguments)
    try:
        validate_result(function, result)
    except DeclarationError as exc:
        raise ExecutionError(
            "return_type", "Polars producer must return one DataFrame or LazyFrame"
        ) from exc
    lazy = result.lazy() if isinstance(result, pl.DataFrame) else result
    logical = logical_schema(pl, lazy.collect_schema())
    if declaration.output.schema is not None and logical != declaration.output.schema:
        raise ExecutionError(
            "output_schema",
            "Output schema differs from its declaration; no implicit casts are applied",
        )
    send({"type": "phase", "phase": "materializing"})
    output = directory / "part-00000.parquet"
    # Create-new preserves existing files. IO sink uses the streaming engine; a
    # bounded row group is not a universal memory cap for arbitrary query shapes.
    with output.open("xb") as stream:
        os.chmod(output, 0o600)
        lazy.sink_parquet(
            stream,
            engine="streaming",
            compression=request["compression"],
            row_group_size=row_group,
            maintain_order=True,
        )
        stream.flush()
        os.fsync(stream.fileno())
    # Reopen staged bytes only. The one-row count query does not collect a dataframe.
    actual = logical_schema(pl, pl.read_parquet_schema(output))
    if actual != logical:
        raise ExecutionError("output_schema", "Staged Parquet changed the output logical schema")
    rows = (
        pl.scan_parquet(str(output), glob=False, hive_partitioning=False)
        .select(pl.len())
        .collect(engine="streaming")
        .item()
    )
    with output.open("rb") as stream:
        digest = file_digest(stream).hex
    manifest = {
        "format_version": 1,
        "logical_schema": actual,
        "schema_fingerprint": schema_digest(actual).hex,
        "files": [
            {
                "path": "part-00000.parquet",
                "sha256": digest,
                "byte_length": str(output.stat().st_size),
                "row_count": str(rows),
            }
        ],
        "writer": {
            "engine": "polars",
            "version": pl.__version__,
            "compression": request["compression"],
            "row_group_size": str(row_group),
        },
    }
    artifact_digest(manifest)
    _write(directory / "artifact.json", manifest)
    return manifest
