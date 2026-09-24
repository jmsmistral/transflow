from datetime import date
import polars as pl
from transflow import Output, source_transform


@source_transform(output=Output("raw/orders"))
def orders() -> pl.DataFrame:
    # Synthetic input; a real source can fetch an API inside this function.
    return pl.DataFrame(
        {
            "order_id": [1001, 1002],
            "customer_id": [1, 2],
            "order_date": [date(2026, 9, 18), date(2026, 9, 18)],
            "amount": [1742, 9117],  # Integer minor units, not floating money.
        },
        schema={
            "order_id": pl.Int64,
            "customer_id": pl.Int64,
            "order_date": pl.Date,
            "amount": pl.Int64,
        },
    )
