# Transflow

[![Toolchains & dependencies](https://github.com/jmsmistral/transflow/actions/workflows/qualification.yml/badge.svg?branch=master)](https://github.com/jmsmistral/transflow/actions/workflows/qualification.yml)
[![Rust backend & CLI](https://github.com/jmsmistral/transflow/actions/workflows/rust.yml/badge.svg?branch=master)](https://github.com/jmsmistral/transflow/actions/workflows/rust.yml)
[![Python](https://github.com/jmsmistral/transflow/actions/workflows/python.yml/badge.svg?branch=master)](https://github.com/jmsmistral/transflow/actions/workflows/python.yml)
[![Lineage web UI](https://github.com/jmsmistral/transflow/actions/workflows/web.yml/badge.svg?branch=master)](https://github.com/jmsmistral/transflow/actions/workflows/web.yml)
[![Security & license checks](https://github.com/jmsmistral/transflow/actions/workflows/safety.yml/badge.svg?branch=master)](https://github.com/jmsmistral/transflow/actions/workflows/safety.yml)

A local, code-first build system for dataframe datasets, with versioned Parquet outputs, declarative checks, and an interactive lineage interface.

**Status:** Contributor foundation implemented (T001–T002). T004 provides the ten-crate Rust scaffold and CLI help/version. T005 adds one locally installable `transflow` wheel containing the typed SDK and worker bootstrap modules. T003 native qualification passes on all three supported platforms. T006 adds a web development preview with theme and dialog controls. T007 adds deterministic integration-test fixtures for future execution and storage work. T008 adds dependency/license audits and repository privacy checks. T009 qualifies DuckDB’s internal validation capabilities and records precision/security boundaries. T010 adds a snapshot-bound catalogue/mypy prototype; editor completion qualification is retained for later integration. T011 adds resource, cancellation and offline resolver probes. T012 defines versioned wire schemas and shared Rust/Python/TypeScript compatibility fixtures. T013 adds validated Rust domain identities, paths, branches, schemas and values. T014 adds generated contracts and bounded Rust/Python worker framing with session guards. T015 adds canonical JSON and separate content fingerprints with cross-language golden vectors. T016 adds safe human diagnostics and versioned CLI JSON output for help/version and usage errors. T017 adds pure build/job/attempt state machines, retry history and guarded publication intent. T018 adds the bundled SQLite store, checksummed migrations and bounded repository operations. T019 adds OS-held runtime ownership and validated local coordinator discovery metadata. T020 adds immutable durable catalogue parsing, exact ID/path lookup, aliases/tombstones and foreign read-boundary records. T021 adds workspace initialization and strict configuration/root resolution. T022 adds bounded source allowlists and pre-import module conflict checks. T023 adds Git-free immutable source captures and retained integrity checks. T024 adds optional Git provenance and isolated ref captures without checkout changes. T025 adds explicit hash locking, managed environment synchronization and installed-byte drift checks. T026 adds immutable Python declarations and decorators with explicit return validation. T027 adds isolated captured-source discovery and structured import diagnostics. T028 adds nonpersistent candidate resolution and validated additive registry proposals. T029 adds guarded registry commits and exact-ID crash recovery. T030 adds production snapshot-bound C references and explicit testing contexts. Dataset builds and the coordinator service remain unimplemented; this is not an application release.

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

Workspace initialization is implemented and can be tried from the contributor checkout:

```bash
cargo run --locked -- init /path/to/new-analysis
```

The parent directory must exist. Initialization preserves existing files and creates
a stable workspace UUID, registry, src directory, Polars dependency input and precise
ignore rules. It does not create Git history, execute Python or install packages.
Repeated initialization preserves the UUID and configuration; partial/conflicting
state produces a diagnostic. See the [configuration service](crates/tf-catalog/WORKSPACE.md).

Environment commands are also implemented. Use a Python interpreter prepared with
pip 26.2.1 and pip-tools 7.6.1, and build the local wheel using the contributor
packaging checks before sync:

```bash
cargo run --locked -- --workspace /path/to/new-analysis env lock --python /path/to/tooling/python
cargo run --locked -- --workspace /path/to/new-analysis env sync --python /path/to/tooling/python --runtime-wheel /path/to/transflow-0.0.0.dev0-py3-none-any.whl
cargo run --locked -- --workspace /path/to/new-analysis env check --python /path/to/tooling/python
```

Lock/sync are explicit package operations. The matched Transflow wheel is never
looked up by name on an index. See [environment preparation](crates/tf-exec/ENVIRONMENTS.md)
for offline wheelhouses, target-specific locks and drift checks.

The subsequent build/service commands remain **unimplemented**:

```bash
# Prepare the managed Python environment and write the source modules.
transflow build curated/order_totals
transflow serve --open
```

**Initial delivery focuses on Polars transforms**, with DuckDB used internally for
validation. Pandas and DuckDB SQL transform adapters remain later work. The
[DuckDB feasibility probe](tools/qualification/duckdb/README.md) is contributor
evidence, not an available scratchpad or transform adapter.

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
CI checks only this implementation repository. Specification checks remain part of
the local contributor workflow and do not require CI access to the private spec repo.
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
qualification now passes; T001–T020 are complete for their bootstrap/domain/transport/hash/diagnostic/state-machine/storage/ownership/registry scope. T009 and T011 probes pass
the native platform/Python matrix; T010 retains native editor checks for later integration.
The [catalogue overlay spike](docs/development/catalog-overlay.md) now passes runtime and
mypy checks on both Python versions; T010 is complete for its prototype scope, with native editor checks retained for later integration.
Local verification and remote CI results are recorded separately in the task evidence.
T007’s [three-platform Rust CI run](https://github.com/jmsmistral/transflow/actions/runs/35410393011) also passes.
See the [compatibility matrix and setup](docs/development/compatibility.md)
for the isolated probe environment, measured results and CI workflow.

T012’s [contract definitions](schemas/README.md) pass shared Rust/Python/TypeScript
fixtures and all five [CI workflows](docs/development/evidence/t012-ci.json).
The [T013 domain library](crates/tf-domain/README.md) now implements validated IDs,
paths, branch declarations and lossless values. The [T014 protocol library](crates/tf-protocol/README.md) adds framing and generated
contracts; canonical hashing is next in T015.

T013 passes [local verification](docs/development/evidence/t013-macos-arm64.json)
and all five [CI workflows](docs/development/evidence/t013-ci.json).

CLI options and the current JSON/error contract are documented in the
[CLI guide](crates/transflow/README.md). `--json` is available for help/version;
unavailable commands still fail clearly without starting work.

### Python declarations

The SDK now provides `Input`, `Output`, `Check`, `transform` and `source_transform`.
Decorated functions remain directly callable for unit tests. Declarations preserve
branch/fallback policy, immutable checks, typed parameters and source refresh
metadata. See the [authoring guide](python/DECLARATIONS.md) for examples and limits.
The initial expectation constructors cover primary keys and non-null columns; the
complete DSL, build execution and publication remain later tasks.

[Isolated discovery](python/DISCOVERY.md) now collects declarations in fresh workers, without invoking producer functions.

[Candidate reconciliation](crates/tf-catalog/CANDIDATES.md) resolves same-pass outputs and rejects graph conflicts before registry mutation.

[Registry recovery](crates/transflow/REGISTRY_RECOVERY.md) journals exact IDs before guarded file replacement and recovers interrupted indexing without rewriting user edits.

[Snapshot-bound C references](python/CATALOG.md) support explicit aliases, namespace prefixes and separate workspace test contexts without database access.
