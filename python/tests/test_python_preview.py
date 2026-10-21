"""Display-only expansion must resolve actual bindings without calling user code."""

from __future__ import annotations

import ast
from typing import Any

import pytest
from transflow.declarations import get_declaration
from transflow_worker.python_preview import execution_declaration, python_preview

SOURCE = """from transflow import Input, Output, transform_meta
def make(source, target, region):
    @transform_meta(orders=Input(source), output=Output(target))
    def regional(orders, *, output):
        assert output.path == target
        return orders.dataframe().filter(region)
    return regional
"""


def preview(source: str = SOURCE, region: object = "west") -> str | None:
    namespace: dict[str, Any] = {}
    exec(compile(source, "captured.py", "exec"), namespace)
    function = namespace["make"]("raw/orders", "curated/west", region)
    declaration = get_declaration(function)
    assert declaration is not None
    return python_preview(function, declaration, source.encode())


def test_preview_resolves_paths_closure_and_only_the_selected_function() -> None:
    result = preview()
    assert result is not None
    assert "Input('raw/orders')" in result and "Output('curated/west')" in result
    assert "assert output.path == 'curated/west'" in result
    assert ".filter('west')" in result
    assert "def make(" not in result and "return regional" not in result
    ast.parse(result)


def test_preview_does_not_call_transform_and_safely_quotes_captured_strings() -> None:
    source = SOURCE.replace(
        "        assert output.path",
        "        raise RuntimeError('must not run')\n        assert output.path",
    )
    result = preview(source, "west'\nprint('not code')")
    assert result is not None
    tree = ast.parse(result)
    function = next(node for node in tree.body if isinstance(node, ast.FunctionDef))
    assert isinstance(function.body[0], ast.Raise)
    assert "must not run" in result


@pytest.mark.parametrize("region", [None, True, 5, 1.25, ("west", 1)])
def test_immutable_literals_are_supported(region: object) -> None:
    result = preview(region=region)
    assert result is not None
    ast.parse(result)


@pytest.mark.parametrize(
    "body",
    [
        "return [region for region in orders]",
        "return lambda region: region",
        "def nested(region): return region\n        return nested(orders)",
        "nonlocal region\n        region = 'changed'\n        return orders",
    ],
)
def test_unsupported_lexical_scopes_are_unavailable_instead_of_misexpanded(body: str) -> None:
    source = SOURCE.replace("return orders.dataframe().filter(region)", body)
    assert preview(source) is None


def test_decorator_only_factory_locals_are_not_confused_with_same_named_globals() -> None:
    source = SOURCE.replace(
        "def make(source, target, region):",
        "cache = 'never'\ndef make(source, target, region):\n    cache = 'deterministic'",
    ).replace("output=Output(target))", "output=Output(target), cache=cache)")
    assert preview(source) is None


def test_preview_is_optional_and_bounded() -> None:
    assert preview(region="x" * (128 * 1024)) is None
    assert preview(SOURCE + "#" * (512 * 1024)) is None


def test_binding_helpers_are_not_rewritten_as_if_they_were_input_constructors() -> None:
    source = SOURCE.replace(
        "def make(source, target, region):",
        "def bind(path): return Input('prefix/' + path)\ndef make(source, target, region):",
    ).replace("orders=Input(source)", "orders=bind(source)")
    assert preview(source) is None


def test_definition_time_defaults_are_resolved_without_calling_expressions_again() -> None:
    source = SOURCE.replace(
        "def regional(orders, *, output):", "def regional(orders, *, output, ctx=region):"
    )
    assert "ctx='west'" in (preview(source) or "")


def test_display_metadata_is_ignored_for_legacy_execution_matching() -> None:
    old = {"authoring": {"configuration": "hash"}, "function": "selected"}
    new: dict[str, Any] = {
        "authoring": {"configuration": "hash", "python_preview": "display"},
        "function": "selected",
    }
    assert execution_declaration(old) == execution_declaration(new)
    assert new["authoring"]["python_preview"] == "display"
