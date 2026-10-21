"""Create the captured Python/SQL factory review with a real built environment."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import create_alpha_demo as alpha  # noqa: E402

REGIONAL = """from transflow import Check, Input, Output, transform, transform_meta
from transflow import expectations as E
import polars as pl


def make_region(source, target, region):
    @transform(
        output=Output(
            target,
            checks=[
                Check(E.row_count().gt(0), "At least one order"),
                Check(E.col("region").equals(region), "Expected region"),
            ],
        ),
        orders=Input(
            source,
            checks=[
                Check(E.col("amount").gte(0), "Nonnegative amounts"),
            ],
        ),
    )
    def regional(orders):
        return orders.filter(pl.col("region") == region)

    return regional


def make_meta_region(source, target, region):
    @transform_meta(
        output=Output(target),
        orders=Input(source),
    )
    def regional(orders, *, output):
        # These values belong to the accepted plan, including fallback branches.
        assert orders.alias == "orders"
        assert orders.version and orders.dataset_id
        assert orders.schema["fields"][0]["name"] == "id"
        assert output.path == target and output.version is None
        assert output.branch == orders.branch
        return orders.dataframe().filter(pl.col("region") == region)

    return regional
"""
EXPORTS = """from factory_helpers import make_region, make_meta_region

# Collection order and repeated function names do not determine dataset identity.
TRANSFORMS = [
    make_region("raw/orders", "curated/east", "east"),
    make_meta_region("raw/orders", "curated/west", "west"),
]
"""
SQL = '''from transflow import Input, Output, Parameter, sql_transform


def make_summary(source, target):
    @sql_transform(
        output=Output(target),
        deliveries=Input(source),
        params={"minimum_amount": Parameter({"type": "i64"}, {"type": "i64", "value": "0"})},
    )
    def summary(deliveries, *, output):
        assert deliveries.alias == "deliveries"
        assert deliveries.path == source and deliveries.version
        assert deliveries.schema["fields"]
        assert output.path == target and output.version is None
        # Values use DuckDB $parameters, without interpolating them into SQL.
        return """
            SELECT region, COUNT(*) AS order_count,
                   CAST(SUM(amount) AS BIGINT) AS total_amount,
                   ROUND(AVG(amount), 2) AS average_amount
            FROM deliveries
            WHERE amount >= $minimum_amount
            GROUP BY region
            ORDER BY region
        """

    return summary


TRANSFORMS = [make_summary("curated/delivery", "analytics/region_summary")]
'''


def create(root: Path, cli: Path, wheel: Path, wheelhouse: Path) -> dict[str, object]:
    previous = alpha.SOURCES
    alpha.SOURCES = {
        name: source
        for name, source in previous.items()
        if name not in {"east.py", "west.py", "region_summary.py", "region_summary.sql"}
    }
    alpha.SOURCES.update(
        {"factory_helpers.py": REGIONAL, "regional.py": EXPORTS, "region_summary.py": SQL}
    )
    try:
        receipt = alpha.create(root, cli, wheel, wheelhouse)
    finally:
        alpha.SOURCES = previous
    return receipt


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("workspace", type=Path)
    for name in ("cli", "wheel", "wheelhouse"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    print(
        json.dumps(
            create(
                args.workspace.resolve(),
                args.cli.resolve(strict=True),
                args.wheel.resolve(strict=True),
                args.wheelhouse.resolve(strict=True),
            )
        )
    )


if __name__ == "__main__":
    main()
