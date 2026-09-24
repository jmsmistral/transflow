import polars as pl
from transflow import Output, source_transform


@source_transform(output=Output("raw/customers"))
def customers() -> pl.DataFrame:
    return pl.DataFrame(
        {"id": [1, 2], "customer_name": [" Ada ", " Grace "], "age": [0, 199]},
        schema={"id": pl.Int64, "customer_name": pl.String, "age": pl.Int64},
    )
