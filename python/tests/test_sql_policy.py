"""Pure policy guards; actual parser and isolated-engine cases run in native qualification."""

from typing import Any

import pytest
from transflow_worker.sql_policy import SqlPolicyError, _Visitor, view_names


@pytest.mark.parametrize("names", [[], ["x", "X"], ["x.parquet"], ["main.x"], ["x;drop"], ["é"]])
def test_binding_namespace_rejects_ambiguous_or_external_names(names: list[str]) -> None:
    with pytest.raises(SqlPolicyError):
        view_names(names)


@pytest.mark.parametrize(
    "node",
    [
        {"type": "FUTURE_NODE"},
        {"class": "FUTURE", "type": "FUTURE"},
        {"class": "OPERATOR", "type": "FUTURE_OPERATOR"},
        {"type": "FUTURE_TABLE", "query_location": 1},
        {"type": "BASE_TABLE", "table_name": "dataset", "schema_name": "main"},
        {"type": "BASE_TABLE", "table_name": "missing"},
        {"function_name": "read_text"},
        {"function_name": "count", "schema": "main"},
        {"type": "FUTURE_MODIFIER"},
        {"id": "USER"},
        {"type_info": {"type": "ENUM_TYPE_INFO"}},
        {"class": "STAR", "type": "STAR", "columns": True},
    ],
)
def test_unknown_or_unsafe_ast_constructs_fail_closed(node: dict[str, Any]) -> None:
    with pytest.raises(SqlPolicyError):
        _Visitor().visit(node, {"dataset"})


def test_nested_cte_names_do_not_escape_into_sibling_queries() -> None:
    nested = {
        "type": "SELECT_NODE",
        "cte_map": {
            "map": [
                {"key": "hidden", "value": {"query": {"type": "SELECT_NODE"}}},
            ]
        },
    }
    with pytest.raises(SqlPolicyError, match="explicitly bound"):
        _Visitor().visit([nested, {"type": "BASE_TABLE", "table_name": "hidden"}], {"dataset"})


def test_structural_limits_fail_closed() -> None:
    with pytest.raises(SqlPolicyError, match="structural limits"):
        _Visitor().visit([None] * 8192, {"dataset"})
    tree: Any = None
    for _ in range(65):
        tree = [tree]
    with pytest.raises(SqlPolicyError, match="structural limits"):
        _Visitor().visit(tree, {"dataset"})
