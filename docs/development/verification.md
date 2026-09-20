# Contributor verification

The contributor checks use Python's standard library and Git; T004 adds the
pinned Rust toolchain, rustfmt and Clippy; T005 adds hash-locked Python package
tooling in a separate repository-local environment. T006 adds Node 24.4.1/npm
11.4.2 and locked web tooling. Document checks were verified
on macOS 15.3.1 arm64 with Python 3.14.7 from the project's `transflow` pyenv
environment. Python 3.11+ is the checker syntax baseline; other interpreters and
Linux have not yet been run for the document checker. The separate
[T003 compatibility matrix](compatibility.md) records dependency probe results.

## Run the current checks

```bash
cd ~/dev/transflow
cargo fetch --locked  # Explicit first-time setup; requires registry access.
python -m venv target/python/py314
target/python/py314/bin/python -m pip install --require-hashes --only-binary=:all: -r python/dev-py314.lock
npm --prefix web ci --ignore-scripts
bash tools/check.sh
```

The script resolves both checkout paths relative to itself, enters the
implementation root so pyenv selects `.python-version`, and stops on failure.
An optional `PYTHON` environment variable can name another interpreter executable.
It requires the sibling specification checkout for contributor verification.
After explicit setup, checks use locked, offline Cargo resolution. The Rust-only
entry point, `bash tools/check-rust.sh`, works without the sibling specification.
Python package checks also work independently through `bash tools/check-python.sh`;
`PYTHON_CHECK` can select an absolute tooling interpreter path, defaulting to
`target/python/py314/bin/python`. See [Python setup](../../python/README.md).

The checker inventories each repository using Git, checks all tracked and
nonignored candidate Markdown, and checks Python/TOML/JSON fences by parsing them.
It validates local inline links, reference definitions and ATX heading anchors;
remote links are counted without network access. It never runs fenced examples.
The mutation tests use disposable paired repositories and synthetic files.

To get machine-readable documentation results or audit a separate unpacked
public export, invoke the canonical checker directly:

```bash
python -B ../transflow-spec/tools/check_spec.py --json
python -B ../transflow-spec/tools/check_spec.py --export-root /tmp/transflow-public-export
```

The export directory must already exist. Exports are audited separately, without
Git ignore rules. Repository inventory excludes ignored private references;
tracked private paths fail even when their working files have been deleted.
Both checks reject the private reference directory, mapped screenshot basenames,
Finder metadata and symlinks that cannot be safely audited. Original public image
assets are allowed. These are path hygiene checks, not image-content recognition
or a secret scanner: renamed copies and arbitrary embedded private content still
need review and the T120 release checks. T008 adds the bounded credential-pattern
scanner described in the [safety baseline](safety.md). No private image bytes are loaded.

## Extending the aggregate contract

The contributor aggregate runs document validation and 43 tooling regression
tests, followed by eleven Rust dependency-checker tests and the following gates:

```bash
cargo fmt --all -- --check
cargo check --workspace --locked --offline
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo test --workspace --locked --offline
```

The dependency checker examines declared normal, optional, target, build and dev
dependencies by package identity, rejects forbidden edges/cycles, requires exact
pins and verifies external lock entries against T003's probe lockfile. It also
checks that each crate inherits workspace lints. Expanding its allowlists requires
explicit architectural and qualification review.

Five Rust tests cover the CLI: help/version and rejected commands in a subprocess
outside the checkouts with an empty PATH, plus typed stdout/stderr failures.
T013 adds pure domain constructors in `tf-domain`; no storage or runtime service
is implemented. T007 adds sixteen
`tf-store` integration tests for deterministic providers, labelled barriers, real
SQLite/filesystem behavior and supervised direct children; see the [fixture
contracts](../../tests/README.md). The tiny transaction schema is not the product
publication algorithm. Actual SQLite 3.51.3, WAL/FULL settings and foreign keys
are checked; no test substitutes an in-memory map for persistence. The
[Rust CI workflow](../../.github/workflows/rust.yml) runs the same Rust entry point
on macOS arm64 and Linux x86_64/arm64; it does not need the specification checkout.
Its three-platform CI run at `1717a4f` passed.

The aggregate then invokes Python tooling lock verification, `pip check`, Ruff
lint/format, strict mypy and pytest. Package tests build one `transflow` wheel from
explicit source roots copied outside both checkouts. They verify that the wheel
contains both typed modules, has one version/distribution identity and needs no
engine imports or workspace activity for SDK import. Actual pip installation in
a clean environment is offline. Downstream mypy checks use installed wheel types;
invalid argument types fail. Worker tests cover both entry points, unavailable
operations, protocol mismatch, distribution drift and failed output streams.
T010 extends the package suite to 48 tests per interpreter: immutable catalogue
contexts, generation failure paths and concurrent installed-wheel mypy overlays.
See the [catalogue qualification report](catalog-overlay.md); native editor
completion is still unverified.

The [Python CI workflow](../../.github/workflows/python.yml) covers both Python
versions on all three target platforms and preserves wheel hashes and JUnit
reports. Native local execution uses Python 3.14.7 and the available 3.13.0;
CI's Python 3.13.15/3.14.7 matrix passes on all three platforms. T014 replaces bootstrap metadata with protocol 1.0 framing and session guards;
execution operations remain unimplemented.

The aggregate also runs `bash tools/check-web.sh`: exact Node/npm versions,
strict TypeScript, ESLint, Prettier, six Vitest/jsdom component tests and two Vite
production builds compared byte-for-byte. Reports go to `target/web/`. The
[web workflow](../../.github/workflows/web.yml) runs this on all three target
platforms; all three jobs passed at `1717a4f`. Browser checks use only Codex’s internal
Browser and the [observed checklist](browser-checks.md); no browser driver or
binaries are installed. The component runner does not verify native dialog focus.
T012 adds [authored contracts](../../schemas/README.md), exposed by `tf-protocol`,
and 156 shared schema cases plus 17 version cases. Each language also rejects an
unknown schema rule. Rust runs three contract tests (24 Rust tests total), Python
runs 222 tests, and Vitest runs 180 tests: 174 Node contract tests plus six jsdom
component tests. No browser installation is involved. The generated-contract
registry now includes T014’s four generated outputs with deterministic drift
checks. The T014 protocol library adds framing and session validation; HTTP API
operations remain unimplemented.

Run `bash tools/qualification/check.sh` separately with its prepared Python
environment for T003 native dependency probes; the compatibility guide documents
explicit setup. That runner now also executes the sixteen-test
[T009 DuckDB spike](../../tools/qualification/duckdb/README.md) in a supervised
helper and retains its capability report. The contributor aggregate lints this
probe but does not run engine tests without their separate qualification environment.
The native runner also executes the ten-test [T011 resource/resolver spike](../../tools/qualification/resources/README.md) and retains `resource-capabilities.json`. It exercises independent phase clocks, real process-group cleanup and offline hash-lock installs; production supervision remains unimplemented.
Checks never fetch external dependencies; Python tests install
the locally built wheel into disposable environments. Add broader
integration/release checks as their real manifests and runners become available.
T008 now runs `bash tools/check-safety.sh` in the aggregate: 26 failure/success
regressions, credential scanning of both explicit repositories, nine-lock license
coverage, bounded-age advisory evidence, expiring exceptions and generated-contract
drift. CI uses `--implementation-only` and checks no private specification files;
three standalone CLI regressions cover absent-spec success, retained local-spec
requirements and credential rejection. See [safety setup and limitations](safety.md). Add canonical fixtures,
integration/recovery and browser gates with their implementations. A missing required runner must fail,
not silently count as a pass. Explicit dependency setup remains separate.

The executable tests establish that help/version need no specification checkout,
Python or Git; installed Python bootstrap tests need neither repository. No pipeline correctness, release archive
contents, engine compatibility or full acceptance scenario is established by
this script. The [task evidence](../../../transflow-spec/TASKS.md) and
[current verification report](../../../transflow-spec/VERIFICATION.md) record
what was actually run. Review changes in both repositories before committing;
record each revision separately when those commits exist.

## T012 completion evidence

On 2026-09-19 the full contributor aggregate passed on macOS arm64, including
222 Python 3.14.7 tests; the separate Python 3.13.0 runner also passed 222 tests.
[Local evidence](evidence/t012-macos-arm64.json) records the commands/counts. All
five [CI workflows and their 19 jobs](evidence/t012-ci.json) passed for implementation
`e97eee3bb82d4f921745840e259742d7bec06f97`, paired with specification progress
`22c5f53`. The Rust/web jobs cover all three platforms; Python and native
qualification cover both Python minor versions on each platform. T012 is complete
for contract definitions; production domain/transport/hashing work remains T013–T015.

## T013 domain verification

T013 adds 15 constructor/invariant tests plus a compile-fail identity-role test.
Three tf-protocol tests project shared scalar/schema/catalogue fixtures through
the production constructors. See the [domain API and limits](../../crates/tf-domain/README.md).
The Rust total is 42 ordinary tests and one documentation test; the eleven boundary
regressions still run. No third-party dependency or version was introduced. Only
the allowed tf-protocol test dependency on tf-domain was added. The full contributor aggregate passes (44 document regressions, 26 safety
regressions, eleven Rust boundary regressions, 42 Rust tests plus one compile-fail
test, 222 Python and 180 web tests). [Local evidence](evidence/t013-macos-arm64.json)
and [all five CI workflows / 19 successful jobs](evidence/t013-ci.json) qualify
implementation `51c6bd7`, paired with specification progress `c15071b`. T013 is
complete; production codecs/framing and canonical hashing remain T014–T015.

## T014 transport and generated contracts

The [protocol guide](../../crates/tf-protocol/README.md) describes strict 1 MiB
framing, 64-level raw JSON nesting limits, session guards and deterministic exports.
The Rust runner now needs the prepared `python` interpreter for a real Python peer
on a private Unix socket (`TRANSFLOW_TEST_PYTHON` overrides it). An environment that
blocks Unix sockets must grant local IPC permission; that test must not be skipped.
The installed wheel includes schema data, named validators and framing helpers; it
requires neither repository nor third-party Python dependencies. SDK/worker metadata
now reports protocol 1.0 with no executable operations. The full contributor aggregate passes: 44 document regressions, 27 safety regressions,
eleven Rust boundary regressions, 52 Rust tests plus one compile-fail documentation
test, 403 Python tests and 182 web tests. Both local Python 3.14.7 and 3.13.0
runners pass. The shared corpus now includes integral decimal metadata written
as JSON `3.0`/`-2.0` (157 schema cases plus 17 version cases).
[Local evidence](evidence/t014-macos-arm64.json) records the scope. All five
[CI workflows and their 19 jobs](evidence/t014-ci.json) pass at implementation
`36bee7164c92cd66a93160456b007241e4b34917`, paired with specification progress
`072167dd2243bb67103a03229242b1ee1d36d702`. T014 is complete.

## T015 canonical encoding and hashing

The [canonical profile](../../schemas/canonical-v1.md) defines exact byte rules and
purpose/version prefixes. Shared vectors test 21 encodings, 105 content digests and
five invalid numeric values in Rust/Python/TypeScript. Targeted tests cover limits,
streamed raw hashing, artifact schema integrity, catalogue tampering and explicit
semantic/presentation selection. Publication UUIDs remain independent.

The already locked `sha2` 0.10.9 is now a direct tf-protocol/native probe dependency;
the native probe checks the standard SHA-256 `abc` answer. No package identity or
version was added. The full aggregate passes: 44 document regressions, 27 safety
regressions, eleven boundary regressions, 56 Rust tests plus one compile-fail
documentation test, 442 Python tests and 210 web tests. Both local Python 3.14.7
and 3.13.0 package runners pass. Native qualification passes using the prepared
engine environment. The final fixture correction was rechecked by all three
languages and both Python versions. See [local evidence](evidence/t015-macos-arm64.json);
All five [CI workflows and their 19 jobs](evidence/t015-ci.json) pass at implementation
`34506fa27a0c10c98f3830daf9b8a73b833494e8`, paired with specification progress
`d09ce720917e9ec7ed3f55ae614fc916fcae8d08`. T015 is complete.

## T016 safe diagnostics and CLI output

The full `bash tools/check.sh` aggregate passes: 44 document regressions, 27 safety
regressions, eleven Rust boundary regressions, 61 Rust tests plus one compile-fail
documentation test, 462 Python tests and 220 web tests. Both local Python 3.14.7
and 3.13.0 runners pass. The shared corpus now has 167 cases, including diagnostic
text and result-envelope failures; five generated outputs reproduce exactly.
The TypeScript fixture reader now compares array constants structurally. The
renamed-checkout document test preserves links to the intra-repository CLI crate.
[Local evidence](evidence/t016-macos-arm64.json) records scope. All five
[CI workflows](evidence/t016-ci.json) pass at `492ebc3a72ebe79afb6785e59eeaea6dbc26b4b4`;
specification progress `62c7d0b5a498ea503c14250825852d88d91d3a4f` is pushed. Browser verification
was used while the public API was rate-limited. T016 is complete.

## T035 local import preparation verification (2026-09-20)

The full contributor aggregate passes: 182 Rust tests plus one documentation test,
635 Python tests, 250 web tests, 27 installed-worker CLI tests, 44 document,
27 safety and twelve boundary regressions. The subsequent readability refactor
also passes workspace Clippy and all nine candidate tests. The dependency inventory
remains 640 versions, with no new versions and zero advisory matches after refresh.
The shared contract corpus now contains 196 cases. See the [local evidence](evidence/t035-macos-arm64.json)
and [import guide](../../crates/transflow/IMPORTS.md). All five [CI workflows and 19 jobs](evidence/t035-ci.json) pass at implementation `8451e4fa215d0ee1126c1d22c1804bb8af50e75f`, paired with specification progress `ec3ba66530a8c15b89077ce0579d4d7b754f85ff`. T035 preparation is complete; no normalization, quality-check PASS, artifact or dataset publication is claimed.

## T036 logical normalization verification (2026-09-20)

The full contributor aggregate passes. Final Rust verification (after the added
schema-forgery regression) passes 190 tests plus one documentation test. Python
passes 635 tests, web 250, installed-worker CLI 28, document regressions 44,
safety 27 and boundary checks twelve. The production-normalizer engine probe
passes seven groups on both prepared local Python versions using PyArrow 25.0.1,
Polars 1.44.2 and DuckDB 1.5.5. No dependency version or lock changed.
See [local evidence](evidence/t036-macos-arm64.json) and the
[normalization guide](../../crates/tf-store/NORMALIZATION.md). All five [CI workflows and 19 jobs](evidence/t036-ci.json) pass at implementation `c99f4c2517a557f95fdd7ba6ce38797550d10225`, paired with specification progress `13477efd082c7a71ea9d237ef6ed03a3a25d9b42`. The six native jobs also pass the production-normalizer engine probe. T036 is complete.
Publication and quality evaluation remain later tasks; pandas/SQL transforms remain deferred.

## T037 immutable artifact verification (2026-09-20)

`bash tools/check.sh` passes: 196 Rust tests plus one documentation test,
635 Python, 250 web and 28 CLI tests; 44 document, 27 safety and twelve
boundary regressions. Six new artifact tests cover ordered/empty files,
no-replace deduplication, corruption, containment and durability-boundary errors.
See [local evidence](evidence/t037-macos-arm64.json) and the
[artifact guide](../../crates/tf-store/ARTIFACTS.md). All five workflows and 19 jobs
pass at `c1c4513c8ac58cb9e427945dfeea3b6aeea02c75`, paired with specification
progress `7520ba5`; see [CI evidence](evidence/t037-ci.json). T037 is complete.

## T038 publication verification (2026-09-20)

`bash tools/check-rust.sh` passes: 205 Rust tests plus one documentation test
and twelve boundary regressions. Nine new publication integration tests exercise
actual SQLite, Parquet and runtime ownership. They cover atomic readers/rollback,
exact input/check evidence, WARN/error handling, cancellation/head conflicts,
complete reservations, deduplicated bytes with distinct versions, recovery and
stable outbox replay. See [local evidence](evidence/t038-macos-arm64.json) and
the [service guide](../../crates/tf-store/PUBLICATION.md). All five workflows
and 19 jobs pass at `b541b1545c9fd605fe3435616f310af30187f8f7`, paired with
specification progress `67b10d0`; see [CI evidence](evidence/t038-ci.json).
T038 is complete.

## T039 crash/recovery verification (2026-09-20)

The full contributor aggregate and final Rust checks pass: 211 Rust tests plus one
documentation test, 635 Python, 250 web and 28 CLI tests; 44 document, 27 safety
and twelve boundary regressions. Six recovery test entrypoints cover nine real
SIGKILL boundaries, eighteen returned-I/O-error cases, corruption, async ownership
and same-build partial success. Replay succeeds after original sources are removed.
See [local evidence](evidence/t039-macos-arm64.json) and the
[recovery report](publication-recovery.md). All five workflows and 19 jobs
pass at `b8a60ad4055ff37ed9c6b81d1adcf769cea7f55c`, paired with specification
progress `8bb90f4`; see [CI evidence](evidence/t039-ci.json). T039 is complete.

## T040 read-lease verification (2026-09-20)

`bash tools/check-rust.sh` passes: 217 Rust tests plus one documentation test
and twelve boundary regressions. Six retention tests cover exact head pins,
renewal/release fencing, provider-owner restart, expiry/clock rollback, transitive
roots and collection/publication exclusion. See [local evidence](evidence/t040-macos-arm64.json)
and the [retention guide](../../crates/tf-store/RETENTION.md). After correcting the
CLI fixture and released-lease lifecycle integration (see T041 below), all five
workflows and 19 jobs pass at `fe65878a151ab34d9c6ebcaf17bff85159289fd2`, paired
with specification progress `48a54a4004e6bf68825d9656acb49db38acb2dde`; see
[CI evidence](evidence/t040-ci.json). T040 is complete.


## T041 output-branch verification (2026-09-20)

Contributor checks pass through Rust, Python and web: 222 Rust tests plus one
documentation test, 635 Python and 250 web tests; 44 document, 27 safety and twelve
boundary regressions. The CLI stage exposed an older positional lease fixture
that no longer matched schema 4; naming its columns fixes it, and the complete
CLI rerun passes all 28 tests. The same fixture failed the T040 Rust CI workflow;
this failure is retained here rather than reported as a successful aggregate.
Final full Rust checks also pass, including a regression that released leases no
longer block catalogue lifecycle changes. Five new branch tests cover selection priority and actual Git states, nonmutating
preview, tombstones/revisions, empty creation, audit rollback and validation/owner
requirements. See [local evidence](evidence/t041-macos-arm64.json) and the
[branch guide](../../crates/transflow/BRANCHES.md). All five workflows and 19 jobs
pass at `fe65878a151ab34d9c6ebcaf17bff85159289fd2`, paired with specification
progress `48a54a4004e6bf68825d9656acb49db38acb2dde`; see
[CI evidence](evidence/t041-ci.json). T041 is complete.


## T042–T043 input resolution verification (2026-09-20)

`bash tools/check.sh` passes: 234 Rust tests plus one documentation test,
635 Python, 250 web and 28 CLI tests; 44 document, 27 safety and twelve boundary
regressions. The final strengthened fingerprint assertions also pass the seven
input integration tests and targeted Clippy; rustfmt passes. Twelve new tests
cover all six selector cases, nonrecursive/empty policy rules, captured validation
context, planned/off-branch boundaries, exact pins through head advances, corrupt
heads with a healthy fallback, collection conflict, failed checks preserving the
last output, and repeated alias/role/version provenance through publication.

See [T042 local evidence](evidence/t042-macos-arm64.json),
[T043 local evidence](evidence/t043-macos-arm64.json) and the
[input service guide](../../crates/tf-store/INPUTS.md). All five workflows and 19
jobs pass at `9ec1436a0d9ed2786753752a2a8f203afa8c086e`, paired with specification
progress `98b85edf9c64f16c2b94d8534b88b1e324ad7c59`; see
[T042 CI evidence](evidence/t042-ci.json) and [T043 CI evidence](evidence/t043-ci.json).
Both tasks are complete.
Full build/plan CLI, provider resolution, freshness and check execution remain
later tasks; these results qualify the shared local input services.


## T044–T045 branch lifecycle and historical reads (2026-09-20)

`bash tools/check.sh` passes: 250 Rust tests plus one documentation test,
641 Python, 253 web and 28 CLI tests; 44 document, 27 safety and twelve boundary
regressions. Sixteen new Rust tests cover audited lifecycle/tombstones, policy
snapshots, active references, actual Git merge/deletion independence, full-length
names and safe terminal display, exact selector overrides, alias ambiguity,
wrong-version/write conflicts, replay immutability, all-or-none leases and corrupt
original data with a healthy current head. The shared contract matrix has 199 cases.

Earlier runs caught stale inventory/advisory/generation records and the fixture
count assertion; all are synchronized and final checks pass. OSV reports zero
matches across 640 locked versions. No dependency version changed.

See [T044 local evidence](evidence/t044-macos-arm64.json),
[T045 local evidence](evidence/t045-macos-arm64.json), the
[branch guide](../../crates/transflow/BRANCHES.md) and
[pin/replay guide](../../crates/tf-store/REPLAY.md). All five workflows and 19 jobs pass at `08c81b95fb099c3eb482b7b72d780c86c0ceef4b`, paired with specification progress `0446b4647956c7af4b7c4f0b093525a56f5515b3`. See [T044 CI evidence](evidence/t044-ci.json) and [T045 CI evidence](evidence/t045-ci.json). Both tasks are complete.
Public branch lifecycle is available; public plan/build/replay execution remains
later work. Replay metadata is distinct from source/environment execution readiness.


The initial native CI at `5211253ff92aa94d766d5442392a8606e27e1e99` failed
with exit 101 in the [native probe step](https://github.com/jmsmistral/transflow/actions/runs/35514068655).
A local standalone `tf-store` build reproduced missing Serde derive macros:
workspace feature unification had hidden its undeclared feature requirement.
The crate now enables `derive` explicitly, and normal Rust checks include the
standalone normalization example. `bash tools/check-rust.sh` passes all 250 tests
plus one documentation test after the fix; full native qualification also passes
with the existing `target/qualification/py314` environment. Initial attempts with
the SDK-only Python environments stopped on missing DuckDB; no dependencies were
installed. Replacement-commit CI passes; see the task evidence above.


## T046–T050 planning and compute keys (2026-09-20)

All local component gates pass: 263 Rust tests plus one documentation test,
641 Python, 253 web and 36 installed-worker integration journeys. The document,
safety and Rust-boundary suites pass 44, 27 and twelve regressions respectively.
Thirteen new Rust tests include exhaustive four-node DAG selection, refresh clock
and force boundaries, real SQLite/Parquet guarded acceptance and rollback, atomic
replay retention, and conservative fingerprint invalidation. Eight new integration
journeys use actual captured imports and managed environments without running
producer functions. Unknown/missing inputs, source/registry/environment drift,
proposed IDs, symbolic parents and source policies are covered.

The initial aggregate stopped at formatting/type errors in the new Python probe
helper. These were corrected; `bash tools/check-python.sh`, `bash tools/check-web.sh`
and `bash tools/check-cli.sh` then passed. Previously passing Rust/document/safety
gates did not need repetition for the Python annotation fix. The local evidence
records that distinction. The lockfile adds only the existing planner crate to
application composition; no dependency version changed. Inventory/advisory evidence
was refreshed: zero OSV matches across 640 locked package versions.

See [T046 local evidence](evidence/t046-macos-arm64.json),
[T047 local evidence](evidence/t047-macos-arm64.json),
[T048 local evidence](evidence/t048-macos-arm64.json),
[T049 local evidence](evidence/t049-macos-arm64.json),
[T050 local evidence](evidence/t050-macos-arm64.json), and the
[planning guide](../../crates/transflow/PLANNING.md).
All five CI workflows and 19 jobs passed at `3e89b3118d897f95e8b6880d09da6abff364720d`,
paired with specification progress `44b914c8418c88ff304075cb3427df8dad8e72b0`.
See the [T050 CI receipt](evidence/t050-ci.json); T046–T049 retain matching receipts.

The services are local preparation foundations; public dispatch/execution, cache
adoption, provider replication and boundary currentness retain subsequent tasks.

## T051 branch-scoped cache reuse (2026-09-21)

Full `bash tools/check.sh` passes: 273 Rust tests plus one documentation test,
641 Python tests, 253 web tests, 41 installed-worker CLI journeys and 44/27/12
document/safety/boundary regressions. Ten new real SQLite/Parquet tests cover
unchanged reuse, older-head adoption, original WARN/input-alias evidence, branch
isolation, guard/expiry/integrity refusals, SQL failure rollback, recovery and
publication receipt readback after adoption. Five installed-worker journeys cover
accepted-context finalization, pending parents and force/source/cache-never rules.
The initial sandbox-only cache run could not bind the existing coordinator socket;
the authorized native rerun and final aggregate pass. No dependency versions changed.
See [local evidence](evidence/t051-macos-arm64.json) and the [cache guide](../../crates/tf-store/CACHE.md).
All five workflows and 19 jobs pass for implementation
`6ea3ce24eb64cdb6b80d7ef7224e97942a66d577`, paired with specification progress
`f63a59e062808c3d3df3c85b47fc94bbe70142b7`. The [CI receipt](evidence/t051-ci.json)
records every job conclusion across macOS arm64 and Linux x86_64/arm64.
No public build or canonical evaluator execution is claimed.
