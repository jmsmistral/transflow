import polars as pl
from transflow import Check, Input, Output, transform
from transflow import expectations as E
from common.names import clean_name


valid_age = E.all(E.col("age").non_null(), E.col("age").gte(0), E.col("age").lt(200))


@transform(
    orders=Input("raw/orders"),
    customers=Input(
        "raw/customers",
        checks=[
            Check(E.primary_key("id"), "Customer IDs are unique and present",
                  id="customer_pk", on_error="FAIL"),
            Check(valid_age, "Customer age is valid", id="customer_age", on_error="FAIL"),
        ],
    ),
    output=Output(
        "curated/customer_orders",
        checks=Check(E.primary_key("order_id"), "Order IDs are unique and present",
                     id="order_pk", on_error="FAIL"),
    ),
)
def customer_orders(orders: pl.LazyFrame, customers: pl.LazyFrame) -> pl.LazyFrame:
    return (
        orders.join(customers, left_on="customer_id", right_on="id", how="left")
        .select(
            "order_id", "customer_id", clean_name("customer_name").alias("customer_name"),
            "order_date", "amount",
        )
    )
