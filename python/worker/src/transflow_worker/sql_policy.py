"""Fail-closed policy over the pinned DuckDB parser, separate from trusted transform SQL."""

from __future__ import annotations

import json
import re
from typing import Any

from .environment import MANAGED_PACKAGES


class SqlPolicyError(ValueError):
    """A safe diagnostic that never includes SQL, parameters, paths or engine messages."""


# Each function is a pure calculation over bound values, not an engine/environment reader.
# Keep this deliberately reviewed list small; do not infer safety from catalog metadata.
FUNCTIONS = frozenset(
    {
        "count",
        "count_star",
        "sum",
        "min",
        "max",
        "avg",
        "abs",
        "round",
        "floor",
        "ceil",
        "lower",
        "upper",
        "length",
        "substring",
        "trim",
        "ltrim",
        "rtrim",
        "concat",
        "isnan",
        "isfinite",
        "isinf",
        "date_part",
        "date_trunc",
        "year",
        "month",
        "day",
        "row_number",
        "rank",
        "dense_rank",
        "lag",
        "lead",
        "first_value",
        "last_value",
        "+",
        "-",
        "*",
        "/",
        "//",
        "%",
        "~~",
        "!~~",
        "~~*",
        "!~~*",
    }
)
EXPRESSIONS = {
    "COLUMN_REF": {"COLUMN_REF"},
    "CONSTANT": {"VALUE_CONSTANT"},
    "STAR": {"STAR"},
    "FUNCTION": {"FUNCTION"},
    "COMPARISON": {
        "COMPARE_EQUAL",
        "COMPARE_NOTEQUAL",
        "COMPARE_LESSTHAN",
        "COMPARE_GREATERTHAN",
        "COMPARE_LESSTHANOREQUALTO",
        "COMPARE_GREATERTHANOREQUALTO",
        "COMPARE_DISTINCT_FROM",
        "COMPARE_NOT_DISTINCT_FROM",
    },
    "CONJUNCTION": {"CONJUNCTION_AND", "CONJUNCTION_OR"},
    "OPERATOR": {
        "OPERATOR_IS_NULL",
        "OPERATOR_IS_NOT_NULL",
        "OPERATOR_NOT",
        "OPERATOR_COALESCE",
        "COMPARE_IN",
        "COMPARE_NOT_IN",
        "GROUPING_FUNCTION",
    },
    "CAST": {"OPERATOR_CAST"},
    "CASE": {"CASE_EXPR"},
    "PARAMETER": {"VALUE_PARAMETER"},
    "WINDOW": {
        "WINDOW_AGGREGATE",
        "WINDOW_ROW_NUMBER",
        "WINDOW_RANK",
        "WINDOW_RANK_DENSE",
        "WINDOW_LAG",
        "WINDOW_LEAD",
        "WINDOW_FIRST_VALUE",
        "WINDOW_LAST_VALUE",
    },
    "SUBQUERY": {"SUBQUERY"},
    "BETWEEN": {"COMPARE_BETWEEN", "COMPARE_NOT_BETWEEN"},
}
TYPES = {
    "SQLNULL",
    "BOOLEAN",
    "TINYINT",
    "SMALLINT",
    "INTEGER",
    "BIGINT",
    "HUGEINT",
    "UTINYINT",
    "USMALLINT",
    "UINTEGER",
    "UBIGINT",
    "UHUGEINT",
    "FLOAT",
    "DOUBLE",
    "DECIMAL",
    "VARCHAR",
    "BLOB",
    "DATE",
    "TIME",
    "TIMESTAMP",
    "TIMESTAMP_SEC",
    "TIMESTAMP_MS",
    "TIMESTAMP_NS",
    "TIMESTAMP_TZ",
    "INTERVAL",
    "INTEGER_LITERAL",
    "STRING_LITERAL",
}
MAX_SQL_BYTES = 16 * 1024


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SqlPolicyError(message)


def view_names(names: list[str]) -> set[str]:
    """Use a small ASCII identifier namespace, case-insensitive like DuckDB identifiers."""
    require(1 <= len(names) <= 32, "Expected one to 32 explicit version bindings")
    require(
        all(re.fullmatch(r"[A-Za-z_][A-Za-z_0-9]{0,62}", name) for name in names),
        "Dataset aliases must be simple ASCII identifiers",
    )
    folded = {name.lower() for name in names}
    require(len(folded) == len(names), "Dataset aliases must be unique ignoring case")
    return folded


def validate_query(db: Any, sql: str, bindings: list[str]) -> None:
    """Validate exactly one SELECT/WITH; never execute authored SQL during validation.

    Serialization uses the very engine that will execute the statement, avoiding a
    second parser's dialect disagreements. Unknown statement, expression, function,
    relation and type constructs fail closed. Settings still provide defence in depth.
    """
    tables = view_names(bindings)
    require(
        isinstance(sql, str) and 0 < len(sql.encode()) <= MAX_SQL_BYTES and "\x00" not in sql,
        "SQL must be nonempty and at most 16 KiB",
    )
    try:
        # VERSION is trusted setup SQL. A changed parser needs explicit requalification.
        version = db.execute("SELECT version()").fetchone()[0]
        require(version == "v" + MANAGED_PACKAGES["duckdb"], "Unqualified SQL parser version")
        statements = db.extract_statements(sql)
        require(
            len(statements) == 1 and statements[0].type.name == "SELECT",
            "Exactly one SELECT/WITH statement is required",
        )
        raw = db.execute("SELECT json_serialize_sql(?)", [sql]).fetchone()[0]
        require(len(raw) <= 1024 * 1024, "SQL AST exceeds the serialization limit")
        parsed = json.loads(raw)
        require(
            not parsed.get("error") and len(parsed.get("statements", [])) == 1,
            "SQL syntax is outside the supported analytical subset",
        )
        _Visitor().visit(parsed["statements"][0]["node"], tables)
    except SqlPolicyError:
        raise
    except Exception:
        raise SqlPolicyError("SQL could not be parsed by the qualified analytical parser") from None


class _Visitor:
    def __init__(self) -> None:
        self.remaining = 8192

    def visit(self, value: Any, tables: set[str], depth: int = 0) -> None:
        self.remaining -= 1
        require(self.remaining >= 0 and depth <= 64, "SQL AST exceeds structural limits")
        if isinstance(value, list):
            for child in value:
                self.visit(child, tables, depth + 1)
            return
        if not isinstance(value, dict):
            return
        kind = value.get("type")
        if isinstance(kind, str) and kind.endswith("_NODE"):
            require(kind == "SELECT_NODE", "Only SELECT query nodes are supported")
            require(value.get("sample") is None, "Sampling is outside the SQL subset")
            # CTE names are scoped, never collected globally from unrelated subqueries.
            entries = value.get("cte_map", {}).get("map", [])
            names = [entry["key"] for entry in entries]
            if names:
                ctes = view_names(names)
                tables = tables | ctes
            require(
                not any(entry["value"].get("key_targets") for entry in entries),
                "Recursive CTE keys are unsupported",
            )
        if "query_location" in value and "class" not in value:
            require(
                kind in {"BASE_TABLE", "JOIN", "SUBQUERY", "EMPTY"},
                "Only registered views, joins and subqueries may supply rows",
            )
            require(value.get("sample") is None, "Table sampling is unsupported")
        if kind == "BASE_TABLE":
            require(
                value["table_name"].lower() in tables
                and not value.get("schema_name")
                and not value.get("catalog_name")
                and value.get("at_clause") is None,
                "Only explicitly bound views and local CTEs are allowed",
            )
        if kind == "JOIN":
            require(value.get("ref_type") in {"REGULAR", "CROSS"}, "Unsupported join form")
        if "class" in value:
            require(
                kind in EXPRESSIONS.get(value["class"], set()),
                "Expression is outside the reviewed SQL subset",
            )
            if value["class"] == "STAR":
                require(
                    not any(
                        value.get(key) for key in ("columns", "replace_list", "expr", "rename_list")
                    ),
                    "Dynamic column expansion is unsupported",
                )
        if "function_name" in value:
            require(
                value["function_name"].lower() in FUNCTIONS
                and not value.get("schema")
                and not value.get("catalog")
                and not value.get("export_state"),
                "Function is outside the reviewed allowlist",
            )
        if "id" in value:
            require(value["id"] in TYPES, "Type is outside the reviewed SQL subset")
        if "type_info" in value and value["type_info"] is not None:
            info = value["type_info"]
            require(
                info.get("type") == "DECIMAL_TYPE_INFO"
                and not info.get("alias")
                and info.get("extension_info") is None,
                "Custom, nested and extension types are unsupported",
            )
        if isinstance(kind, str) and kind.endswith("_MODIFIER"):
            require(
                kind in {"ORDER_MODIFIER", "LIMIT_MODIFIER", "DISTINCT_MODIFIER"},
                "Query modifier is outside the reviewed SQL subset",
            )
        for child in value.values():
            self.visit(child, tables, depth + 1)
