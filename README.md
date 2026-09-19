# Transflow

[![Native qualification](https://github.com/jmsmistral/transflow/actions/workflows/qualification.yml/badge.svg?branch=master)](https://github.com/jmsmistral/transflow/actions/workflows/qualification.yml)
[![Rust](https://github.com/jmsmistral/transflow/actions/workflows/rust.yml/badge.svg?branch=master)](https://github.com/jmsmistral/transflow/actions/workflows/rust.yml)
[![Python](https://github.com/jmsmistral/transflow/actions/workflows/python.yml/badge.svg?branch=master)](https://github.com/jmsmistral/transflow/actions/workflows/python.yml)
[![Web](https://github.com/jmsmistral/transflow/actions/workflows/web.yml/badge.svg?branch=master)](https://github.com/jmsmistral/transflow/actions/workflows/web.yml)
[![Safety](https://github.com/jmsmistral/transflow/actions/workflows/safety.yml/badge.svg?branch=master)](https://github.com/jmsmistral/transflow/actions/workflows/safety.yml)

A local, code-first build system for dataframe datasets, with versioned Parquet outputs, declarative checks, and an interactive lineage interface.

**Status:** Contributor foundation implemented (T001–T002). T004 provides the ten-crate Rust scaffold and CLI help/version. T005 adds one locally installable `transflow` wheel containing the typed SDK and worker bootstrap modules. T003 native qualification passes on all three supported platforms. T006 adds a web development preview with theme and dialog controls. T007 adds deterministic integration-test fixtures for future execution and storage work. T008 adds dependency/license audits and repository privacy checks. Transform declarations, dataset builds and the coordinator remain unimplemented; this is not an application release.

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

The Python distribution is named **transflow**, containing both `transflow` (SDK) and `transflow_worker` (worker). See [local wheel installation and checks](python/README.md). The intended public install is `pip install transflow` after publication; no package has been published yet. The Rust binary remains a separate native artifact.

## Specification and contributor setup

Keep the canonical specification in a sibling checkout directly under `~/dev/`. These are independent repositories; the shared parent is not a Transflow project or scan root:

```text
~/dev/
  transflow/       # this implementation repository
  transflow-spec/  # authoritative design, task ledger and agent guidance
```

Read the [specification document map](../transflow-spec/README.md) and [engineering agreement](../transflow-spec/AGENTS.md) before implementation. [AGENTS.md](AGENTS.md) is the short agent entry point. The specification is also hosted at [jmsmistral/transflow-spec](https://github.com/jmsmistral/transflow-spec). Detailed specification files are not duplicated here.

Specification baseline: **1.1.1**. See its task ledger for intended scope; unchecked tasks are not delivered features. End users of a future installed release will not need the specification checkout.

## Development checks

Use the project-local pyenv environment, Git and the pinned Rust 1.98.1 toolchain
with rustfmt and Clippy. Document checks use the Python standard library (3.11+);
package checks use a separate, hash-locked Python 3.14 tooling environment.
Web checks require Node 24.4.1 and npm 11.4.2.
Prepare dependencies once before running the offline checks:

```bash
cd ~/dev/transflow
cargo fetch --locked
python -m venv target/python/py314
target/python/py314/bin/python -m pip install --require-hashes --only-binary=:all: -r python/dev-py314.lock
npm --prefix web ci --ignore-scripts
bash tools/check.sh
```

This checks specification metadata, task dependencies/evidence, references,
example syntax and public file paths; it then runs tooling regressions, Rust
dependency checks, formatting, compilation, Clippy, Ruff, mypy and bootstrap tests.
Web gates check TypeScript, ESLint, component tests and repeatable production assets.
The [safety baseline](docs/development/safety.md) checks credentials, locked dependency
licenses, advisory evidence, expiring exceptions and registered generated contracts.
Advisory refresh is explicit and required after seven UTC calendar days; CI queries current data.
Tests build a wheel and install it into disposable environments without network
access; external dependency installation remains explicit. See the
[verification contract](docs/development/verification.md) for scope and limitations.

Try the development CLI after fetching dependencies:

```bash
cargo run --locked --offline -p transflow -- --help
cargo run --locked --offline -p transflow -- --version
```

Only help and version are available. Other commands fail with a diagnostic.
The built executable works outside either checkout without Python or Git.
`bash tools/check-rust.sh` runs the Rust checks without the sibling specification.

Try the [web preview](web/README.md) with `npm --prefix web run dev`. Browser
verification uses Codex’s internal Browser; no browser installation is needed.
The preview does not connect to a coordinator or execute dataset operations.

The [test infrastructure](tests/README.md) provides virtual time, deterministic
IDs/randomness, controlled failure barriers, real SQLite/filesystem fixtures and
supervised test children. It does not implement dataset publication or workers.

The initial source boundaries are [crates/](crates/README.md),
[python/](python/README.md) and [web/](web/README.md). T003 has pinned the initial
toolchain/dependency candidates and exercised them on macOS arm64. Native Linux
qualification now passes; T001–T008 are complete for their bootstrap scope.
Local verification and remote CI results are recorded separately in the task evidence.
T007’s [three-platform Rust CI run](https://github.com/jmsmistral/transflow/actions/runs/35410393011) also passes.
See the [compatibility matrix and setup](docs/development/compatibility.md)
for the isolated probe environment, measured results and CI workflow.
