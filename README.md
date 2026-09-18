# Transflow

A local, code-first build system for dataframe datasets, with versioned Parquet outputs, declarative checks, and an interactive lineage interface.

**Status:** Contributor foundation implemented (T001–T002). T004's ten-crate Rust scaffold builds locally and provides CLI help/version output. T003 native qualification is running in CI, with results pending. Dataset builds, the coordinator, Python SDK and frontend remain unimplemented; this is not an application release.

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

These commands describe the intended workflow; they are **not implemented yet**:

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

Use the project-local pyenv environment, Git and the pinned Rust 1.98.1 toolchain
with rustfmt and Clippy. Python checks use only the standard library (3.11+).
Fetch the locked Cargo dependencies once before running the offline checks:

```bash
cd ~/dev/transflow
cargo fetch --locked
bash tools/check.sh
```

This checks specification metadata, task dependencies/evidence, references,
example syntax and public file paths; it then runs tooling regressions, Rust
dependency checks, formatting, compilation, Clippy and scaffold tests. The check
command does not install dependencies. See the
[verification contract](docs/development/verification.md) for scope and limitations.

Try the development CLI after fetching dependencies:

```bash
cargo run --locked --offline -p transflow -- --help
cargo run --locked --offline -p transflow -- --version
```

Only help and version are available. Other commands fail with a diagnostic.
The built executable works outside either checkout without Python or Git.
`bash tools/check-rust.sh` runs the Rust checks without the sibling specification.

The initial source boundaries are [crates/](crates/README.md),
[python/](python/README.md) and [web/](web/README.md). T003 has pinned the initial
toolchain/dependency candidates and exercised them on macOS arm64. Native Linux
qualification remains pending, so T003 and T004 remain unchecked in the task ledger.
See the [compatibility matrix and setup](docs/development/compatibility.md)
for the isolated probe environment, measured results and CI workflow.
