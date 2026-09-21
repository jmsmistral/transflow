"""Lossless frozen parameter conversion at the execution-only Python boundary."""

from datetime import datetime
from decimal import Decimal
from types import MappingProxyType
from typing import Any, cast

import pytest
from transflow_worker.polars_adapter import Context, ExecutionError, _value


def test_parameter_conversion_keeps_large_integers_decimal_scale_and_nested_immutability() -> None:
    value = _value(
        {
            "type": "struct",
            "fields": [
                {"name": "id", "value": {"type": "u64", "value": "18446744073709551615"}},
                {
                    "name": "amount",
                    "value": {"type": "decimal", "value": "123.450", "precision": 10, "scale": 3},
                },
                {"name": "list", "value": {"type": "list", "values": [{"type": "null"}]}},
            ],
        }
    )
    assert isinstance(value, MappingProxyType)
    assert value["id"] == 18446744073709551615
    assert value["amount"] == Decimal("123.450")
    assert str(value["amount"]) == "123.450"
    assert value["list"] == (None,)
    assert _value({"type": "f32", "value": "0.1"}) == 0.10000000149011612
    assert _value({"type": "f64", "value": "0.1"}) == 0.1
    with pytest.raises(TypeError):
        cast(Any, value)["id"] = 0


def test_submicrosecond_parameters_fail_instead_of_python_datetime_truncation() -> None:
    timestamp = {
        "type": "timestamp",
        "unit": "ns",
        "timezone": "UTC",
        "value": "2026-09-21T00:00:00.123456789Z",
    }
    with pytest.raises(ExecutionError, match="submicrosecond"):
        _value(timestamp)
    timestamp["value"] = "2026-09-21T00:00:00.123456000Z"
    converted = _value(timestamp)
    assert isinstance(converted, datetime) and converted.microsecond == 123456
    assert converted.tzinfo is not None


def test_context_never_resolves_secrets_from_ambient_environment(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("synthetic_secret", "not-a-secret-delivery-mechanism")
    context = Context(MappingProxyType({}), "build", "job", "attempt", datetime(2026, 9, 21), None)
    with pytest.raises(ExecutionError, match="source execution service"):
        context.secret("synthetic_secret")
