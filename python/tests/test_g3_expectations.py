"""Pure G3 builder contracts, type separation and declared alias checks."""

import pytest
from transflow import Check, DeclarationError, Input, Output, transform
from transflow import expectations as E


@pytest.mark.parametrize(
    "expression",
    [
        E.col("value").approx(1.0, abs_tol=0.01, rel_tol=0.01),
        E.col("value").null_count().equals(0),
        E.col("value").null_fraction().lte(0.1),
        E.col("value").distinct_count().gt(0),
        E.group_by("key", "other").count().lte(2),
        E.col("label").matches("[a-z]+", mode="full"),
        E.col("stamp").age_seconds_at(E.evaluation_time()).lte(3600),
        E.schema().equals({"format_version": 1, "fields": []}),
        E.col("key").in_dataset("reference", column="id"),
        E.row_count().equals(E.input("previous").row_count()),
    ],
)
def test_roundtrip(expression: E.Expectation) -> None:
    wire = expression.to_wire()
    assert E.Expectation.from_wire(wire).to_wire() == wire


@pytest.mark.parametrize("tolerance", [-1, float("nan"), float("inf"), "1"])
def test_bad_tolerances_are_not_data_violations(tolerance: object) -> None:
    with pytest.raises(DeclarationError):
        E.col("value").approx(1.0, abs_tol=tolerance, rel_tol=0.0)


def test_count_stays_integer_and_fraction_stays_numeric() -> None:
    with pytest.raises(DeclarationError):
        E.row_count().equals(1.5)
    E.col("value").null_fraction().equals(0.5)
    with pytest.raises(DeclarationError):
        E.group_by("key").count().equals(1.5)
    with pytest.raises(DeclarationError):
        E.all(E.col("label").matches("x", mode="partial"), E.row_count().gt(0))


def test_membership_alias_is_declared_and_validation_is_not_a_function_argument() -> None:
    check = Check(E.col("key").in_dataset("reference", column="id"), "Known key")
    with pytest.raises(DeclarationError, match="undeclared"):
        transform(rows=Input("raw/items"), output=Output("curated/items", checks=check))

    @transform(
        rows=Input("raw/items"),
        reference=Input("raw/reference", role="validation"),
        output=Output("curated/items", checks=check),
    )
    def produce(rows: object) -> object:
        return rows

    assert produce(42) == 42
