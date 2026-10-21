"""Explicit factory discovery, immutable configuration and handle API boundaries."""

from __future__ import annotations

from dataclasses import FrozenInstanceError
from pathlib import Path
from typing import Any

import pytest
from test_discovery import request, run_worker
from test_packaging import installed as installed
from test_packaging import wheel as wheel
from transflow import Input, InputDataset, Output, OutputDataset, sql_transform, transform_meta
from transflow.declarations import get_declaration
from transflow_worker.registration import configuration


def test_meta_and_sql_decorators_do_not_call_user_code() -> None:
    @transform_meta(deliveries=Input("curated/delivery"), output=Output("analytics/meta"))
    def meta(deliveries: Any, *, output: Any) -> Any:
        return deliveries.dataframe()

    @sql_transform(deliveries=Input("curated/delivery"), output=Output("analytics/sql"))
    def sql() -> str:
        return "SELECT * FROM deliveries"

    assert get_declaration(meta).handles  # type: ignore[union-attr]
    assert get_declaration(sql).handles  # type: ignore[union-attr]
    assert sql() == "SELECT * FROM deliveries"


def test_handles_are_readonly_and_schema_mutation_is_isolated() -> None:
    input_dataset = InputDataset(
        "id",
        "curated/delivery",
        "deliveries",
        "review",
        "master",
        "exact",
        b'{"format_version":1,"fields":[]}',
        lambda: "lazy",
    )
    assert input_dataset.dataframe() == "lazy"
    schema = input_dataset.schema
    schema["fields"].append("changed")
    assert input_dataset.schema["fields"] == []
    with pytest.raises(FrozenInstanceError):
        input_dataset.branch = "other"  # type: ignore[misc]
    output = OutputDataset("id", "analytics/summary", "output", "review", None)
    assert output.version is None and output.schema is None
    assert not hasattr(output, "dataframe")


def test_mutable_closure_and_default_are_rejected() -> None:
    captured = [1]

    def producer() -> object:
        return captured

    with pytest.raises(ValueError, match="immutable"):
        configuration(producer)

    def default(value: object = captured) -> object:
        return value

    with pytest.raises(ValueError, match="immutable"):
        configuration(default)


FACTORY = """from transflow import Input, Output, sql_transform, transform_meta

def make_sql(source, target, threshold):
    @sql_transform(deliveries=Input(source), output=Output(target))
    def summary(deliveries, *, output):
        raise AssertionError('SQL generation must not run during discovery')
        return f'SELECT * FROM deliveries WHERE amount > {threshold}'
    return summary

def make_python(source, target):
    @transform_meta(deliveries=Input(source), output=Output(target))
    def summary(deliveries, *, output):
        raise AssertionError('Python must not run during discovery')
    return summary
"""


def test_imported_factory_exports_are_authoritative_and_order_independent(
    installed: Path,
    tmp_path: Path,
) -> None:
    definitions = []
    for index, exported in enumerate(
        [
            "[make_sql('raw/orders', 'analytics/sql', 10), "
            "make_python('raw/orders', 'analytics/python')]",
            "[make_python('raw/orders', 'analytics/python'), "
            "make_sql('raw/orders', 'analytics/sql', 10)]",
        ]
    ):
        root = tmp_path / str(index)
        root.mkdir()
        sources = {
            "src/helpers.py": FACTORY,
            "src/exports.py": "from helpers import make_sql, make_python\nTRANSFORMS = " + exported,
        }
        code, result, _ = run_worker(installed, request(root, sources))
        assert code == 0, result
        definitions.append({d["output"]["ref"]["value"]: d for d in result["definitions"]})
    assert definitions[0] == definitions[1]
    assert len(definitions[0]) == 2
    for declaration in definitions[0].values():
        assert declaration["module"] == "exports"
        assert declaration["authoring"]["definition_path"] == "src/helpers.py"
        assert declaration["authoring"]["exported"] is True


@pytest.mark.parametrize(
    "exported",
    [
        "None",
        "(x for x in [])",
        "[None]",
        "[[make_sql('a','b',1)]]",
        "[make_sql('a','b',1),make_sql('a','b',2)]",
    ],
)
def test_invalid_exports_fail_before_registration(
    installed: Path, tmp_path: Path, exported: str
) -> None:
    code, result, _ = run_worker(
        installed,
        request(
            tmp_path,
            {
                "src/helpers.py": FACTORY,
                "src/exports.py": "from helpers import make_sql\nTRANSFORMS = " + exported,
            },
        ),
    )
    assert code == 1 and result["code"] == "registration", result


def test_factory_configuration_records_computation_values() -> None:
    def make(minimum: int) -> object:
        @sql_transform(output=Output("analytics/summary"))
        def summary() -> str:
            return f"SELECT {minimum} AS minimum"

        return summary

    assert configuration(make(1)) == configuration(make(1))
    assert configuration(make(1)) != configuration(make(2))
