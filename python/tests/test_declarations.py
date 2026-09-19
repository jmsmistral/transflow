"""Pure authoring acceptance boundaries, including ordinary direct Python calls."""

from __future__ import annotations

import inspect
from dataclasses import FrozenInstanceError
from typing import Any

import pytest
from transflow import (
    Branch,
    Check,
    DeclarationError,
    Input,
    Output,
    Parameter,
    TransformContext,
    source_transform,
    transform,
)
from transflow import expectations as E
from transflow._catalog_prototype import CatalogContextError, PrototypeRef, PrototypeSnapshot
from transflow.catalog import C
from transflow.declarations import get_declaration, validate_result
from transflow.testing import catalog_context


def test_callable_identity_annotations_metadata_and_ordinary_calls() -> None:
    def helper(value: int) -> int:
        """A helper can remain in the producing module."""
        return value + 1

    signature, annotations = inspect.signature(helper), helper.__annotations__.copy()
    check = Check(E.primary_key("id"), "Primary key", id="pk")
    inputs = Input("raw/items", checks=check)
    wrapped = transform(value=inputs, output=Output("curated/items", checks=check))(helper)
    assert wrapped is helper
    assert wrapped(1) == 2  # No engine import, checks or runtime lifecycle.
    assert wrapped.__name__ == "helper"
    assert wrapped.__doc__ == "A helper can remain in the producing module."
    assert wrapped.__annotations__ == annotations
    assert inspect.signature(wrapped) == signature
    declaration = get_declaration(wrapped)
    assert declaration is not None
    assert declaration.inputs == (("value", inputs),)
    assert declaration.filename == __file__ and declaration.line > 0
    with pytest.raises(FrozenInstanceError):
        declaration.engine = "pandas"  # type: ignore[misc]
    assert get_declaration(lambda: None) is None


@pytest.mark.parametrize("branch", [None, Branch.CURRENT, "current", "feature/new"])
@pytest.mark.parametrize("stop", [False, True])
def test_branch_and_fallback_are_independent(branch: str | Branch | None, stop: bool) -> None:
    binding = Input("raw/items", branch=branch, stop_branch_fallback=stop)
    assert binding.branch is branch
    assert binding.stop_branch_fallback is stop


def test_c_reference_retains_own_snapshot_across_contexts() -> None:
    snapshot = PrototypeSnapshot(
        "11111111-1111-4111-8111-111111111111",
        (("raw/items", "22222222-2222-4222-8222-222222222222"),),
    )
    with catalog_context(snapshot, expected_fingerprint=snapshot.fingerprint):
        node = C.raw.items
        with pytest.raises(CatalogContextError, match="namespace"):
            Input(C.raw)
    binding = Input(node)
    assert isinstance(binding.ref, PrototypeRef)
    assert binding.ref.fingerprint == snapshot.fingerprint
    assert binding.ref.dataset_id == snapshot.entries[0][1]
    assert Output(node).ref == binding.ref


def test_check_inheritance_ids_and_immutable_sequence() -> None:
    inherited = Check(E.col("amount").non_null(), "Amount present")
    explicit = Check(
        E.primary_key("id"), "Primary key", on_error="WARN", null_policy="fail", sample_rows=0
    )
    checks = [inherited, explicit]
    output = Output("curated/items", checks=checks)
    checks.clear()
    assert output.checks == (inherited, explicit)
    assert inherited.id == "amount_present"
    assert inherited.null_policy is None and inherited.sample_rows is None
    assert inherited.effective(null_policy="ignore", sample_rows=3).null_policy == "ignore"
    assert inherited.effective(null_policy="ignore", sample_rows=3).sample_rows == 3
    assert explicit.effective(null_policy="ignore", sample_rows=3) == explicit
    with pytest.raises(DeclarationError, match="unique"):
        Output("curated/items", checks=[inherited, inherited])
    # The same check ID is allowed under distinct binding scopes.
    transform(
        a=Input("raw/items", checks=explicit),
        b=Input("raw/items", checks=explicit),
        output=Output("out", checks=explicit),
    )(lambda a, b: a)


@pytest.mark.parametrize(
    "options",
    [
        {"on_error": "warn"},
        {"on_error": "IGNORE"},
        {"null_policy": "pass"},
        {"sample_rows": -1},
        {"sample_rows": True},
        {"id": ""},
    ],
)
def test_invalid_check_policies(options: dict[str, Any]) -> None:
    with pytest.raises(DeclarationError):
        Check(E.primary_key("id"), "Check", **options)


@pytest.mark.parametrize("expectation", ["SELECT TRUE", lambda: True, {"kind": "non_null"}])
def test_checks_reject_executable_and_untyped_expectations(expectation: Any) -> None:
    with pytest.raises(DeclarationError, match="typed expectation"):
        Check(expectation, "Invalid")


@pytest.mark.parametrize(
    "options",
    [
        {"version": "any"},
        {"stop_fallback": True},
        {"role": "other"},
        {"branch": ""},
        {"stop_branch_fallback": 1},
        {"checks": "bad"},
    ],
)
def test_wrong_input_keywords_and_values(options: dict[str, Any]) -> None:
    with pytest.raises((TypeError, DeclarationError)):
        Input("raw/items", **options)


@pytest.mark.parametrize(
    "ref", ["", "../secret", "Raw/items", "raw//items", "raw/class", "dataset:no"]
)
def test_reference_syntax(ref: str) -> None:
    with pytest.raises(DeclarationError):
        Input(ref)


def test_schema_copy_and_rejection() -> None:
    schema = {
        "format_version": 1,
        "fields": [{"name": "id", "logical_type": {"type": "i64"}, "nullable": False}],
    }
    output = Output("out", schema=schema)
    schema["fields"] = []
    assert output.schema != schema
    copy: Any = output.schema
    copy["fields"].clear()
    assert output.schema != copy
    with pytest.raises(DeclarationError):
        Output("out", schema={"format_version": 2, "fields": []})
    with pytest.raises(DeclarationError, match="external"):
        Output("external/provider/items")


@pytest.mark.parametrize(
    "function",
    [
        lambda: None,
        lambda extra: extra,
        lambda *args: None,
        lambda **kwargs: None,
        lambda data, /: data,
    ],
)
def test_signature_errors(function: Any) -> None:
    with pytest.raises(DeclarationError):
        transform(data=Input("raw/items"), output=Output("out"))(function)


def test_validation_only_alias_not_in_signature_and_context_not_evaluated() -> None:
    @transform(
        data=Input("raw/items"), lookup=Input("raw/lookup", role="validation"), output=Output("out")
    )
    def function(data: object, *, ctx: TransformContext) -> object:
        return data

    assert get_declaration(function) is not None
    with pytest.raises(DeclarationError, match="role=data"):
        transform(lookup=Input("raw/lookup", role="validation"), output=Output("out"))(
            lambda lookup: lookup
        )
    with pytest.raises(DeclarationError, match="ctx"):
        transform(output=Output("out"))(lambda ctx: ctx)

    def evil(*, ctx: object) -> None:
        pass

    evil.__annotations__["ctx"] = "should_never_execute()"

    assert transform(output=Output("out"))(evil) is evil


def test_sources_refresh_and_no_inputs() -> None:
    @source_transform(
        output=Output("raw/items"), refresh={"ttl_seconds": 10}, secret_refs=["token"]
    )
    def source() -> int:
        return 12

    assert source() == 12
    declaration = get_declaration(source)
    assert declaration is not None
    assert declaration.source and declaration.inputs == () and declaration.refresh == 10
    assert declaration.secret_refs == ("token",)
    assert get_declaration(
        source_transform(output=Output("raw/manual"), refresh="manual")(lambda: 0)
    )
    with pytest.raises(DeclarationError):
        source_transform(output=Output("raw/items"))(lambda dataframe: dataframe)
    with pytest.raises(TypeError):
        options: Any = {"data": Input("raw/items")}
        source_transform(output=Output("out"), **options)


@pytest.mark.parametrize(
    "refresh",
    [{"ttl_seconds": 0}, {"ttl_seconds": -1}, {"ttl_seconds": True}, {"ttl": 10}, "sometimes"],
)
def test_invalid_source_refresh(refresh: Any) -> None:
    with pytest.raises(DeclarationError):
        source_transform(output=Output("out"), refresh=refresh)


def test_ambiguous_unsupported_and_mutable_declarations() -> None:
    @transform(output=Output("out"))
    def function() -> None:
        pass

    with pytest.raises(DeclarationError, match="multiple"):
        transform(output=Output("other"))(function)
    for options in [
        {"engine": "pandas"},
        {"cache": "maybe"},
        {"ctx": Input("raw/items")},
        {"bad-name": Input("raw/items")},
        {"resources": {"wall_timeout_seconds": -1}},
    ]:
        dynamic: Any = options
        with pytest.raises(DeclarationError):
            transform(output=Output("out"), **dynamic)
    outputs: Any = [Output("a"), Output("b")]
    with pytest.raises(DeclarationError, match="one output"):
        transform(output=outputs)
    resources = {"wall_timeout_seconds": 0}
    lineage = {"id": ["data.id"]}
    declared = transform(
        data=Input("raw/items"), output=Output("out"), resources=resources, lineage=lineage
    )(lambda data: data)
    resources["wall_timeout_seconds"] = 99
    lineage["id"].clear()
    metadata = get_declaration(declared)
    assert metadata is not None and metadata.resources == (("wall_timeout_seconds", 0),)
    assert metadata.lineage_json == b'{"id": ["data.id"]}'


def test_typed_parameters_are_lossless_and_frozen() -> None:
    value = {"type": "u64", "value": "18446744073709551615"}
    parameter = Parameter({"type": "u64"}, value)
    value["value"] = "0"
    assert parameter.default == {"type": "u64", "value": "18446744073709551615"}
    fields = [{"name": "limit", "logical_type": {"type": "i64"}, "nullable": False}]
    Parameter(
        {"type": "struct", "fields": fields},
        {"type": "struct", "fields": [{"name": "limit", "value": {"type": "i64", "value": "10"}}]},
    )
    Parameter(
        {"type": "list", "element": fields[0]},
        {"type": "list", "values": [{"type": "i64", "value": "10"}]},
    )
    options = {"limit": parameter}
    function = transform(output=Output("out"), params=options)(lambda: None)
    options.clear()
    metadata = get_declaration(function)
    assert metadata is not None and metadata.params == (("limit", parameter),)
    for logical, default in [
        ({"type": "i64"}, {"type": "string", "value": "1"}),
        ({"type": "f64"}, {"type": "f64", "value": "NaN"}),
    ]:
        with pytest.raises(DeclarationError):
            Parameter(logical, default)


def test_return_validation_requires_declared_producer() -> None:
    with pytest.raises(DeclarationError, match="decorated"):
        validate_result(lambda: None, None)


def test_deferred_annotation_code_is_not_executed_by_decorator() -> None:
    import sys

    namespace: dict[str, Any] = {}
    exec(
        compile(
            "calls = []\ndef annotation():\n    calls.append(1)\n    return int\n"
            "def function(data: annotation()):\n    return data\n",
            "synthetic.py",
            "exec",
            dont_inherit=True,
        ),
        namespace,
    )
    before = list(namespace["calls"])
    decorated = transform(data=Input("raw/items"), output=Output("out"))(namespace["function"])
    assert decorated(3) == 3
    assert namespace["calls"] == before
    assert before == ([] if sys.version_info >= (3, 14) else [1])
