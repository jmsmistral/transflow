"""Lossless Arrow schema projection for the managed DuckDB boundary."""

from __future__ import annotations

from typing import Any

from .polars_adapter import ExecutionError


def quote(name: str) -> str:
    return '"' + name.replace('"', '""') + '"'


def sql_type(logical: dict[str, Any]) -> str:
    """Restore exact declared input types after DuckDB's Parquet reader mapping."""
    primitives = {
        "bool": "BOOLEAN",
        "i8": "TINYINT",
        "i16": "SMALLINT",
        "i32": "INTEGER",
        "i64": "BIGINT",
        "u8": "UTINYINT",
        "u16": "USMALLINT",
        "u32": "UINTEGER",
        "u64": "UBIGINT",
        "f32": "FLOAT",
        "f64": "DOUBLE",
        "string": "VARCHAR",
        "binary": "BLOB",
        "date": "DATE",
    }
    kind = logical["type"]
    if kind in primitives:
        return primitives[kind]
    if kind == "decimal":
        return f"DECIMAL({logical['precision']},{logical['scale']})"
    if kind == "timestamp":
        return (
            "TIMESTAMPTZ"
            if logical["timezone"] is not None
            else {"ms": "TIMESTAMP_MS", "us": "TIMESTAMP", "ns": "TIMESTAMP_NS"}[logical["unit"]]
        )
    if kind == "list":
        return sql_type(logical["element"]["logical_type"]) + "[]"
    if kind == "struct":
        return (
            "STRUCT("
            + ",".join(
                quote(f["name"]) + " " + sql_type(f["logical_type"]) for f in logical["fields"]
            )
            + ")"
        )
    raise ExecutionError("schema", "SQL input type is outside the qualified logical profile")


def logical_schema(schema: Any, pa: Any) -> dict[str, Any]:
    remaining = 4096

    def field(value: Any, depth: int = 0, *, element: bool = False) -> dict[str, Any]:
        nonlocal remaining
        remaining -= 1
        if depth > 24 or remaining < 0 or value.metadata:
            raise ExecutionError(
                "schema", "SQL schema exceeds supported limits or contains engine metadata"
            )
        return {
            "name": "element" if element else value.name,
            "logical_type": kind(value.type, depth),
            "nullable": value.nullable,
        }

    def kind(value: Any, depth: int) -> dict[str, Any]:
        for name, predicate in [
            ("bool", pa.types.is_boolean),
            ("i8", pa.types.is_int8),
            ("i16", pa.types.is_int16),
            ("i32", pa.types.is_int32),
            ("i64", pa.types.is_int64),
            ("u8", pa.types.is_uint8),
            ("u16", pa.types.is_uint16),
            ("u32", pa.types.is_uint32),
            ("u64", pa.types.is_uint64),
            ("f32", pa.types.is_float32),
            ("f64", pa.types.is_float64),
            ("date", pa.types.is_date32),
        ]:
            if predicate(value):
                return {"type": name}
        if pa.types.is_string(value) or pa.types.is_large_string(value):
            return {"type": "string"}
        if pa.types.is_binary(value) or pa.types.is_large_binary(value):
            return {"type": "binary"}
        if pa.types.is_decimal128(value) and 0 <= value.scale <= value.precision <= 38:
            return {"type": "decimal", "precision": value.precision, "scale": value.scale}
        if pa.types.is_timestamp(value) and value.unit in {"ms", "us", "ns"}:
            if value.tz == "UTC" and value.unit == "ms":
                raise ExecutionError(
                    "schema", "SQL aware milliseconds require explicit microsecond conversion"
                )
            if value.tz is not None and (value.tz != "UTC" or value.unit == "ns"):
                raise ExecutionError(
                    "schema",
                    "DuckDB cannot preserve this timestamp precision or timezone; cast explicitly",
                )
            return {"type": "timestamp", "unit": value.unit, "timezone": value.tz}
        if pa.types.is_list(value) or pa.types.is_large_list(value):
            return {"type": "list", "element": field(value.value_field, depth + 1, element=True)}
        if pa.types.is_struct(value):
            return {"type": "struct", "fields": [field(v, depth + 1) for v in value]}
        raise ExecutionError(
            "schema", "SQL type is outside the lossless logical profile; cast explicitly"
        )

    if schema.metadata:
        raise ExecutionError("schema", "SQL schema metadata requires explicit removal")
    names = [value.name.casefold() for value in schema]
    if len(set(names)) != len(names):
        raise ExecutionError("schema", "DuckDB input/output columns cannot collide by case")
    return {"format_version": 1, "fields": [field(value) for value in schema]}
