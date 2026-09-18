# Transflow

A local, code-first build system for dataframe datasets, with versioned Parquet outputs, declarative checks, and an interactive lineage interface.

**Status:** Design and implementation planning. This README is an initial implementation-repository scaffold, not an announcement of an available release. The specification bundle contains no working Transflow application, installable binary or Python SDK. Replace this status only when implementation and release evidence support the change.

## Intended experience

Write Python transforms, declare their input and output datasets, and build a target without managing a separate catalogue namespace for every source directory. Transflow is designed to validate the dependency graph, resolve exact input versions, run checks, preserve the previous successful output when a build fails, and explain what needs rebuilding.

The intended architecture is a Rust coordinator/CLI with Python workers for Polars, pandas and DuckDB, SQLite metadata, immutable Parquet files, and a local browser UI. Git will be supported but not required. Registered datasets from other workspaces will be consumed as read boundaries rather than automatically executing another workspace's code.

An example of the proposed authoring API is:

```python
import polars as pl
from transflow import Input, Output, transform

@transform(
    orders=Input("raw/orders"),
    output=Output("curated/order_totals"),
)
def order_totals(orders: pl.LazyFrame) -> pl.LazyFrame:
    return orders.group_by("customer_id").agg(pl.col("amount").sum())
```

These commands describe the intended workflow; they do **not** work from this documentation scaffold alone:

```bash
transflow init
# Prepare the managed Python environment and write the source modules.
transflow build curated/order_totals
transflow serve --open
```

Installation commands, verified package names, supported release versions and actual development commands will be added as the corresponding tasks are implemented. Do not install an unrelated registry package merely because it shares the project name.

## Specification and contributor setup

Keep the canonical specification in a sibling checkout:

```text
parent/
  transflow/       # this implementation repository
  transflow-spec/  # authoritative design, task ledger and agent guidance
```

Read `../transflow-spec/README.md` for the document map and `../transflow-spec/AGENTS.md` before implementation. `AGENTS.md` in this repository is the short agent entry point. Detailed specification files are not duplicated here. The exact hosted repository link should be added when it exists; none is invented in this scaffold.

Specification baseline: **1.1.0**. See its task ledger for intended scope; unchecked tasks are not delivered features. End users of a future installed release will not need the specification checkout.
