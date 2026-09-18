# Transflow

A local, code-first build system for dataframe datasets, with versioned Parquet outputs, declarative checks, and an interactive lineage interface.

**Status:** Contributor foundation implemented (T001–T002). Repository structure and automated specification checks are available. There is no working Transflow application, installable binary, Python SDK or frontend yet.

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

Installation commands, verified package names and supported release versions will be added as the corresponding tasks are implemented. Do not install an unrelated registry package merely because it shares the project name.

## Specification and contributor setup

Keep the canonical specification in a sibling checkout directly under `~/dev/`. These are independent repositories; the shared parent is not a Transflow project or scan root:

```text
~/dev/
  transflow/       # this implementation repository
  transflow-spec/  # authoritative design, task ledger and agent guidance
```

Read the [specification document map](../transflow-spec/README.md) and [engineering agreement](../transflow-spec/AGENTS.md) before implementation. [AGENTS.md](AGENTS.md) is the short agent entry point. The specification is also hosted at [jmsmistral/transflow-spec](https://github.com/jmsmistral/transflow-spec). Detailed specification files are not duplicated here.

Specification baseline: **1.1.0**. See its task ledger for intended scope; unchecked tasks are not delivered features. End users of a future installed release will not need the specification checkout.

## Development checks

Use the existing project-local pyenv environment. The contributor checker uses
only the Python standard library (Python 3.11 or newer) and Git:

```bash
cd ~/dev/transflow
bash tools/check.sh
```

This checks specification metadata, task dependencies/evidence, references,
example syntax and public file paths, then runs the checker's regression tests.
It does not install packages or run Transflow application tests. See the
[verification contract](docs/development/verification.md) for scope and limitations.

The initial source boundaries are [crates/](crates/README.md),
[python/](python/README.md) and [web/](web/README.md). T003 next qualifies and pins
toolchains and dependencies before the Rust, SDK/worker and frontend build tasks.
