"""Canonical AST syntax and declaration safety, without an evaluation engine."""

import json
from dataclasses import FrozenInstanceError
from pathlib import Path
from typing import Any, cast

import pytest
from transflow import Check, Input, Output, transform
from transflow import expectations as E
from transflow._declaration_values import DeclarationError
from transflow_worker.canonical import DigestKind, canonical_json, content_digest

CASES = json.loads(
    (Path(__file__).resolve().parents[2] / "schemas/fixtures/expectations-v1.json").read_text()
)


@pytest.mark.parametrize("case", CASES, ids=[c["name"] for c in CASES])
def test_shared_canonical_ast(case: dict[str, Any]) -> None:
    if not case["valid"]:
        with pytest.raises(DeclarationError):
            E.Expectation.from_wire(case["ast"]).validate_inputs(set(case["aliases"]))
        return
    ast = E.Expectation.from_wire(case["ast"])
    ast.validate_inputs(set(case["aliases"]))
    assert ast.to_wire() == case["ast"]
    assert canonical_json(ast.to_wire()).decode() == case["canonical"]
    check = Check(ast, "Example").effective()
    effective = {
        "expectation": ast.to_wire(),
        "null_policy": check.null_policy,
        "sample_rows": str(check.sample_rows),
        "on_error": check.on_error,
    }
    assert content_digest(DigestKind.COMPUTE, effective).hex == case["effective_compute_digest"]


def test_typed_builders_and_promotion() -> None:
    row = E.col("age").non_null()
    compare = E.compare(E.col("age"), "gte", E.literal({"type": "i64", "value": "0"}))
    assert isinstance(E.all(row, compare), E.RowPredicate)
    assert isinstance(E.not_(E.any(row, compare)), E.RowPredicate)
    assert isinstance(E.all(E.primary_key("id"), E.every(row)), E.DatasetExpectation)
    with pytest.raises(DeclarationError, match="Mixed"):
        E.any(row, E.primary_key("id"))
    with pytest.raises(DeclarationError):
        E.all()
    with pytest.raises(DeclarationError):
        bool(row)
    with pytest.raises(DeclarationError):
        Check(cast(Any, E.row_count()), "Not boolean")
    with pytest.raises(DeclarationError):
        E.compare(E.row_count(), "gt", E.col("age"))


def test_immutable_and_no_closure_or_sql() -> None:
    original = {"ast_version": 1, "kind": "non_null", "columns": ["age"]}
    ast = E.Expectation.from_wire(original)
    original["columns"] = ["changed"]
    result = ast.to_wire()
    result["columns"] = ["changed-again"]
    assert ast.to_wire()["columns"] == ["age"]
    with pytest.raises(FrozenInstanceError):
        cast(Any, ast)._json = b"{}"
    for value in ("SELECT TRUE", lambda: True, object()):
        with pytest.raises(DeclarationError):
            E.literal(value)
        with pytest.raises(DeclarationError):
            E.Expectation.from_wire(value)


def test_decorators_validate_aliases_without_running_function_or_predicate() -> None:
    condition = E.compare(E.row_count(), "equals", E.input("reference").row_count())

    def function(rows: object) -> object:
        raise AssertionError("Producer must not execute during decoration")

    decorated = transform(
        rows=Input("raw/rows"),
        reference=Input("raw/reference", role="validation"),
        output=Output("curated/rows", checks=Check(condition, "Counts")),
    )(function)
    assert decorated is function
    with pytest.raises(DeclarationError, match="undeclared"):
        transform(
            rows=Input("raw/rows"), output=Output("curated/rows", checks=Check(condition, "Counts"))
        )(function)


def test_node_budget_rejects_broad_tree() -> None:
    with pytest.raises(DeclarationError, match="limit"):
        E.all(*(E.col("id").non_null() for _ in range(1000)))


def test_nested_json_key_order_and_depth_fail_without_recursive_branch_explosion() -> None:
    node: dict[str, object] = {"kind": "non_null", "columns": ["id"]}
    for _ in range(8):
        node = {"child": node, "kind": "row_not"}
    assert isinstance(E.Expectation.from_wire({"ast_version": 1, **node}), E.RowPredicate)
    for _ in range(32):
        node = {"child": node, "kind": "row_not"}
    with pytest.raises(DeclarationError):
        E.Expectation.from_wire({"ast_version": 1, **node})
