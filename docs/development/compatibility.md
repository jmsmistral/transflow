# Dependency qualification — T003

Status on 2026-09-19: native macOS arm64 and Linux x86_64/arm64 qualification
passes across Python 3.13.15/3.14.7 in CI. T003 is complete. This is a dependency baseline, not an application release
or a claim that the full G0 gate has passed.

The [retained machine-readable report](evidence/t003-macos-arm64.json) contains
the native SQLite/engine results, exact tool output and selected wheel filenames/hashes.

## Selected versions

| Component | Pin / qualification |
|---|---|
| Rust toolchain | 1.98.1; edition 2024; initial MSRV 1.98.1 (older compilers unqualified) |
| Python | 3.14.7 primary; existing 3.13.0 additionally exercised locally; CI pins 3.13.15 and 3.14.7 |
| Resolver | pip 26.2.1 + pip-tools 7.6.1, with their transitive hash lock |
| Polars | 1.44.2, including matching polars-runtime-32 |
| DuckDB | 1.5.5 |
| PyArrow | 25.0.1 |
| pandas / NumPy | 3.0.6 / 2.5.3 |
| Rust Arrow / Parquet | 60.0.0 |
| SQLx / libsqlite3-sys | 0.9.0 / 0.37.0, bundled SQLite enabled |
| Observed Rust SQLite | 3.51.3; source ID below |
| Node / npm | 24.4.1 / 11.4.2, observed installed versions; no global Node update |
| React / React DOM | 19.3.0 |
| React Flow / ELK | @xyflow/react 12.11.6 / elkjs 0.12.0, with the declaration fix below |
| TypeScript / Vite | 6.0.3 / 8.3.0 |

The complete direct Rust pins and transitive resolution live in
[Cargo.toml](../../tools/qualification/rust/Cargo.toml) and
[Cargo.lock](../../tools/qualification/rust/Cargo.lock). Python inputs and hash
locks are under [tools/qualification/python](../../tools/qualification/python/requirements.in);
the [web manifest](../../tools/qualification/web/package.json) and
[npm lock](../../tools/qualification/web/package-lock.json) record the web graph.
These standalone probes inform T004–T006. T004 now provides a separate
[Rust application scaffold](../../crates/README.md); T005 now packages both Python
modules in one local [transflow wheel](../../python/README.md). No package named
Transflow was fetched from an index.

## Results and boundaries

| Target | Python wheels | Native execution |
|---|---|---|
| macOS arm64, macOS 15 | 3.13 and 3.14: eight hash-locked wheel candidates each | Pass on macOS 15.3.1 with Python 3.13.0 and 3.14.7 |
| Linux x86-64, glibc 2.28+ wheel baseline | 3.13 and 3.14: eight candidates each | Passed in CI on both pinned interpreters |
| Linux arm64, glibc 2.28+ wheel baseline | 3.13 and 3.14: eight candidates each | Passed in CI on both pinned interpreters |

Wheel resolution is not native execution, full environment-marker validation or
an OS support promise. The CI jobs use Ubuntu 24.04 and macOS 15. Earlier macOS,
musl Linux, Windows and free-threaded Python have not been qualified. The local
3.13.0 run is a compatibility spot check using an existing interpreter, not a
recommendation to install that old patch release. CI uses the maintained
[3.13.15 release](https://www.python.org/downloads/release/python-31315/).

The Rust executable performs eight checks: actual linked SQLite version/source
identity, WAL/FULL/foreign-key pragmas, transaction rollback, an Arrow/Parquet
round trip, advisory-lock contention/release, an Axum/Tower request, a timezone
DST gap, and Serde/JSON Schema generation. The wrapper also verifies that a
nonempty output directory is rejected. This is not publication/recovery testing.

Each Python native run checks Polars' lazy Parquet sink, PyArrow reads, exact
DuckDB values, unknown-column rejection, Arrow-backed pandas without an implicit
index, empty-file schema preservation and reading the Rust-written Parquet file.
The fixture covers integers beyond JavaScript's exact range, decimal128 values,
nanosecond timestamps, nulls and NaN. Full schema/engine conformance, timezone
precision, subprocess control, check isolation and release wheel qualification
remain later tasks. DuckDB reported its own native memory limit as 102.3 GiB on
this host; the probe did not impose a Transflow memory budget.

The web probe passes strict TypeScript checking, five Node tests (React imports,
connected ELK layout, invalid-edge rejection, idempotent type preparation and
declaration/version drift rejection) and a Vite build with a separate
layout-worker asset. It is not a browser interaction/accessibility test. The build
reports React Flow's `use client` directive warning; this probe is a client-only
bundle, with no React Server Components boundary. The warning remains visible.

## SQLite identity

The built Rust probe reports:

```text
SQLite 3.51.3
2026-03-13 10:38:09 737ae4a34738ffa0c3ff7f9bb18df914dd1cad163f28fd6b6e114a344fe6d618
```

This satisfies the specification's [WAL-reset fix requirement](https://sqlite.org/wal.html).
SQLx uses `sqlite-bundled`, without enabling extension loading. Its allowed
binding range excludes libsqlite3-sys 0.38.2, so the tested pin is 0.37.0. Both
the Cargo lock and runtime assertion protect this choice. Python's stdlib SQLite
reports 3.43.2; it is informational here and is not the Rust control-plane DB.
Updating the system SQLite CLI would not change either linked library.

## ELK declaration correction

Unmodified elkjs 0.12.0 fails strict TypeScript checking because it indexes the
optional `children` property as `T['children'][number]`. Version 0.11.1 and 0.10.2
showed the same failure. [patch-elk.mjs](../../tools/qualification/web/patch-elk.mjs)
changes just that expression to `NonNullable<T['children']>[number]` in the
installed 0.12.0 declaration. It checks the package version and expected expression,
is idempotent, and rejects unexpected content. Runtime JavaScript is untouched;
`skipLibCheck` and broad diagnostic suppression are not enabled.

The correction is an explicit preparation step after `npm ci`, including in CI.
Reassess it on every ELK upgrade and remove it when upstream declarations are fixed.
The [upstream declaration](https://github.com/kieler/elkjs/blob/master/typings/elk-api.d.ts)
and actual strict compiler failure motivated this local patch. TypeScript 6 is the
initial compiler baseline; TypeScript 7 is not qualified by the final probe.

## Reproduce locally

Use the project's Python 3.14 pyenv environment to create an isolated probe
environment. Setup downloads dependencies explicitly; repeated verification does
not install packages. Rust 1.98.1 must be installed by its exact toolchain name.

```bash
cd ~/dev/transflow
python -m venv target/qualification/py314
target/qualification/py314/bin/python -m pip install --require-hashes --only-binary=:all: -r tools/qualification/python/resolver.lock
target/qualification/py314/bin/python -m pip install --require-hashes --only-binary=:all: -r tools/qualification/python/py314.lock
npm --prefix tools/qualification/web ci --ignore-scripts --no-audit --no-fund
npm --prefix tools/qualification/web run prepare:types
cargo fetch --locked --manifest-path tools/qualification/rust/Cargo.toml
PYTHON="$PWD/target/qualification/py314/bin/python" bash tools/qualification/check.sh
```

Use a separately created Python 3.13 environment and `py313.lock` for the second
interpreter. The two engine locks currently match byte-for-byte, but remain
separate because resolution occurs under each actual interpreter. Rust's compiler,
Python's interpreter and Node/npm are explicit prerequisites; the script diagnoses
toolchain drift and does not upgrade them. Dependency probes leave only ignored
reports/artifacts beneath `target/qualification/`. Your project pyenv packages and
system SQLite were not modified by this qualification run.

To repeat the wheel-availability check (network access required):

```bash
target/qualification/py314/bin/python tools/qualification/python/wheel_matrix.py --output target/qualification/wheel-matrix.json
```

The lock-generation command used under each supported interpreter was
`python -m piptools compile --generate-hashes --strip-extras --no-header --no-emit-index-url --no-emit-trusted-host --output-file <lock> tools/qualification/python/requirements.in`.
The resolver lock uses `resolver.in` and additionally `--allow-unsafe` to include
pip/setuptools themselves. See [pip-tools' hash workflow](https://pip-tools.readthedocs.io/en/stable/).
Review changes to inputs, all locks, this matrix and the canonical task evidence
together; do not run automatic dependency upgrades during normal checks.

## Remaining qualification

[The native CI workflow](../../.github/workflows/qualification.yml) is prepared for
six OS/interpreter combinations, with official actions pinned to commit SHAs.
The [six-job run](https://github.com/jmsmistral/transflow/actions/runs/35408977731)
succeeded at `1717a4f` and its public summary was reviewed through the internal Browser.
CI needs no sibling specification
checkout to build or run these probes. It uses read-only repository permissions,
does not publish packages, and uploads measured reports when available.

T003 is complete with the reviewed CI evidence. T005 now builds the single
`transflow` bootstrap wheel with both modules; full release qualification belongs
to T116 and later gates.
No A40/A67 end-to-end acceptance result is claimed. The full CI/security/
license baseline remains T008. Canonical rationale is in
[ADR-001](../../../transflow-spec/docs/adr/ADR-001.md).


## T009 canonical-check feasibility

The [DuckDB capability probe](../../tools/qualification/duckdb/README.md) extends
native qualification with sixteen tests for restricted helper setup, parsed SQL
policy, exact aggregates/typed round trips, interruption and physical spill.
It reuses the existing locked engine environments. Both local Python 3.13.0 and
3.14.7 runs pass; the native runner now writes `duckdb-capabilities.json` alongside
its T003 reports. Remote results for the new tests remain pending.

[ADR-008](../../../transflow-spec/docs/adr/ADR-008.md) records engine limitations
and required guards. Initial authoring is Polars-only; DuckDB remains internal
validation machinery, while Pandas and SQL transform adapters are deferred.
The existing cross-engine probes do not implement those adapters.


## T011 resource and resolver extension

The [resource capability probe](../../tools/qualification/resources/README.md) adds
ten tests to the existing six-job native matrix using unchanged dependency locks.
Local Python 3.13.0/3.14.7 runs pass separate phase deadlines, cancellable disabled
timers, real POSIX group/child reaping, explicit memory policy and offline
pip/pip-tools hash resolution with tamper rejection. Engine defaults and peak
RSS are reported; no universal hard-process memory support is claimed. All six
[native jobs](https://github.com/jmsmistral/transflow/actions/runs/35437047654) passed
at `a46aca6`, using Python 3.13.15/3.14.7 on macOS arm64 and Linux x86_64/arm64.

T032 promotes the already-locked `unicode-ident` 1.0.26 library to an explicit
`tf-catalog` dependency for Python-style XID identifier validation. Its pure
character predicates perform no I/O. The native probe now exercises Unicode
letters, continuation digits and invalid punctuation. The crate boundary/pin
allowlist and both lock inventories record this narrow use; no package version
was added or upgraded. Python NFKC/source-module rules remain enforced by the SDK.
