import polars as pl


def clean_name(column: str) -> pl.Expr:
    """Return a reusable expression; no I/O or side effects at import time."""
    return pl.col(column).str.strip_chars()
