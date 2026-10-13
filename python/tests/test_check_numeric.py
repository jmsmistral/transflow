"""Exact numeric boundaries without loading a data engine."""

import json

from transflow_worker.check_numeric import _age, _decimal, _float


def literal(kind: str, value: str) -> str:
    return json.dumps({"type": kind, "value": value})


def test_approximation_never_turns_overflow_into_pass() -> None:
    target = literal("f64", "-1e308")
    zero = literal("i64", "0")
    assert _float(1e308, target, zero, literal("f64", "1.5")) is False
    assert _float(1e308, target, zero, literal("f64", "2.0")) is True
    assert _float(None, target, zero, zero) is None
    assert _float(float("nan"), target, zero, zero) is False
    assert _float(float("inf"), target, zero, zero) is False


def test_decimal_and_uint64_do_not_round_through_double() -> None:
    zero = literal("i64", "0")
    assert (
        _decimal(
            "18446744073709551615",
            literal("u64", "18446744073709551614"),
            literal("f64", "0.5"),
            zero,
        )
        is False
    )
    assert (
        _decimal(
            "999999999999999999999999999999999999.99",
            json.dumps(
                {
                    "type": "decimal",
                    "value": "999999999999999999999999999999999999.98",
                    "precision": 38,
                    "scale": 2,
                }
            ),
            json.dumps({"type": "decimal", "value": "0.01", "precision": 38, "scale": 2}),
            zero,
        )
        is True
    )


def test_relative_time_compares_exact_microseconds() -> None:
    exact = json.dumps({"type": "decimal", "value": "0.000001", "precision": 6, "scale": 6})
    assert _age("1", exact) is True
    assert _age("2", exact) is False
    assert _age(None, exact) is None
