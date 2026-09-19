"""T009 feasibility helper, not the production evaluator or scratchpad service."""

from __future__ import annotations

import json
from pathlib import Path

import duckdb
import pyarrow as pa


class Unsupported(ValueError):
    """A capability outside the deliberately small qualified prototype."""


def require(condition, message):
    if not condition:
        raise Unsupported(message)


def canonical_file(path: Path) -> str:
    require(
        path.is_absolute() and ".." not in path.parts, "Expected an absolute immutable file path"
    )
    require(not any(p.is_symlink() for p in (path, *path.parents)), "Symlink paths are unsupported")
    require(path.is_file(), "Registered file must exist")
    return str(path.resolve(strict=True))


def validate_schema(schema: pa.Schema) -> None:
    """Reject known lossy inputs before DuckDB reads a canonical check candidate."""
    for field in schema:
        kind = field.type
        if pa.types.is_decimal(kind):
            require(kind.precision <= 38, "DuckDB decimals wider than 38 digits are unsupported")
        elif pa.types.is_timestamp(kind):
            require(
                not (kind.tz is not None and kind.unit == "ns"),
                "Timezone-aware nanosecond timestamps would lose precision",
            )
        else:
            require(
                kind in (pa.int64(), pa.uint64(), pa.float64(), pa.string(), pa.bool_()),
                f"Type not qualified by the T009 prototype: {kind}",
            )


def connect(
    bindings: dict[str, Path],
    temporary: Path,
    *,
    memory_limit=None,
    spill_limit="256MB",
    metadata=None,
):
    paths = {name: canonical_file(path) for name, path in bindings.items()}
    # No application directory allowlist or Python replacement scans. DuckDB adds its
    # owned spill directory when external access is disabled; the probe verifies that scope.
    db = duckdb.connect(
        config={
            "autoinstall_known_extensions": False,
            "autoload_known_extensions": False,
            "allow_community_extensions": False,
            "allow_unsigned_extensions": False,
            "allow_persistent_secrets": False,
            "python_enable_replacements": False,
            "threads": 1,
        }
    )
    try:
        for name, value in {
            "allowed_paths": list(paths.values()),
            "allowed_configs": [],
            "temp_directory": str(temporary),
            "allowed_directories": [],
            "max_temp_directory_size": spill_limit,
            "extension_directory": str(temporary.parent / "extensions"),
            "secret_directory": str(temporary.parent / "secrets"),
            "enable_progress_bar": True,
            "enable_progress_bar_print": False,
            "progress_bar_time": 0,
        }.items():
            db.execute(f"SET {name} = ?", [value])  # Names above are fixed application constants.
        if memory_limit is not None:
            db.execute("SET memory_limit = ?", [memory_limit])
        if metadata is not None:
            metadata["bundled_extensions"] = db.execute(
                "SELECT extension_name FROM duckdb_extensions() "
                "WHERE loaded ORDER BY extension_name"
            ).fetchall()
        db.execute("SET enable_external_access = false")
        for name, path in paths.items():
            # CREATE VIEW cannot bind a filename parameter in DuckDB 1.5.5. Its relation API
            # owns filename/identifier quoting; no user SQL is interpolated into view DDL.
            db.read_parquet(path).create_view(name)
        db.execute("SET lock_configuration = true")
        return db
    except BaseException:
        db.close()
        raise


FUNCTIONS = {
    "count",
    "count_star",
    "sum",
    "min",
    "max",
    "avg",
    "abs",
    "isnan",
    "row_number",
    "+",
    "-",
    "*",
    "/",
    "%",
}
CLASSES = {
    "COLUMN_REF",
    "CONSTANT",
    "STAR",
    "FUNCTION",
    "COMPARISON",
    "CONJUNCTION",
    "OPERATOR",
    "CAST",
    "CASE",
    "PARAMETER",
    "WINDOW",
    "SUBQUERY",
    "BETWEEN",
}
TABLE_TYPES = {"BASE_TABLE", "JOIN", "SUBQUERY", "EMPTY"}
NODE_TYPES = {"SELECT_NODE"}
OPERATORS = {
    "OPERATOR_IS_NULL",
    "OPERATOR_IS_NOT_NULL",
    "OPERATOR_NOT",
    "OPERATOR_COALESCE",
    "COMPARE_IN",
    "COMPARE_NOT_IN",
}


def validate_query(db, sql: str, bindings: set[str]) -> None:
    """Inspect the pinned DuckDB parser's AST; unknown constructs fail closed.

    This spike qualifies SELECT/WITH, filters, aggregates, joins, sorting and a window.
    It is intentionally not the full T079 SQL grammar, type checker or sandbox.
    """
    require(len(sql.encode()) <= 16 * 1024, "Query exceeds prototype text limit")
    statements = db.extract_statements(sql)
    require(
        len(statements) == 1 and statements[0].type == duckdb.StatementType.SELECT,
        "Exactly one SELECT/WITH statement is required",
    )
    parsed = json.loads(db.execute("SELECT json_serialize_sql(?)", [sql]).fetchone()[0])
    require(
        not parsed.get("error") and len(parsed.get("statements", [])) == 1,
        "Unsupported DuckDB SELECT serialization",
    )
    remaining = 2048

    def visit(value, tables, depth=0):
        nonlocal remaining
        remaining -= 1
        require(remaining >= 0 and depth <= 32, "Query AST exceeds prototype limits")
        if isinstance(value, list):
            for child in value:
                visit(child, tables, depth + 1)
        elif isinstance(value, dict):
            kind = value.get("type")
            if isinstance(kind, str) and kind.endswith("_NODE"):
                require(kind in NODE_TYPES, "Query node is outside the qualified subset")
                require(value.get("sample") is None, "Sampling is unsupported")
                ctes = value.get("cte_map", {}).get("map", [])
                tables = tables | {entry["key"].casefold() for entry in ctes}
            if "query_location" in value and "class" not in value:
                require(kind in TABLE_TYPES, "Arbitrary table functions are forbidden")
            if kind == "BASE_TABLE":
                require(
                    value["table_name"].casefold() in tables
                    and not value.get("schema_name")
                    and not value.get("catalog_name")
                    and value.get("at_clause") is None
                    and value.get("sample") is None,
                    "Only explicitly bound dataset views and query CTEs are allowed",
                )
            if "class" in value:
                require(value["class"] in CLASSES, "Expression is outside the qualified subset")
                if value["class"] == "OPERATOR":
                    require(kind in OPERATORS, "Operator is outside the qualified subset")
                if value["class"] == "STAR":
                    require(
                        not value.get("columns")
                        and not value.get("replace_list")
                        and value.get("expr") is None,
                        "Dynamic column expansion is unsupported",
                    )
            if "function_name" in value:
                require(
                    value["function_name"].casefold() in FUNCTIONS
                    and not value.get("schema")
                    and not value.get("catalog")
                    and not value.get("export_state"),
                    "Function is not allowlisted",
                )
            for child in value.values():
                visit(child, tables, depth + 1)

    visit(parsed["statements"][0]["node"], {name.casefold() for name in bindings})


def scratchpad(db, sql: str, bindings: set[str], parameters=None):
    validate_query(db, sql, bindings)
    # Prototype row cap only. Byte-budgeted protocol streaming belongs to T074/T079.
    rows = db.execute(sql, parameters or {}).fetchmany(1001)
    return rows[:1000], len(rows) > 1000
