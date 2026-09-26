# Developer-core qualification (T073)

This matrix scopes specification 1.1.3's developer core to the implemented CLI,
Polars worker, canonical DuckDB checks and local/provider storage services.
It does not qualify the future HTTP/UI, scheduler, additional transform engines,
native editors or release packaging. The owner deferred automatic editor overlays
and native editor qualification to T031/T108. Each scenario below identifies its
executable evidence and any part belonging to a later gate.

## Reproduce

After the explicit dependency setup in [verification.md](verification.md), run
these sequentially from the repository root:

```bash
bash tools/check.sh
PYTHON=target/qualification/py314/bin/python bash tools/qualification/check.sh
```

The first command runs specification checks, privacy/dependency/contract audits,
Rust tests, Python tests, web contract/component tests and installed CLI tests.
The second uses real SQLite, Parquet, Git and supervised processes and retains
native JSON reports under `target/qualification/runs/native.*`. Dependencies must
be prepared beforehand; tests install matched wheels into disposable environments
using local wheels. Individual journey commands are documented in the
[example](../../examples/customer-orders/README.md) and native runner.

Run receipts and measured platform scope are recorded in
[the T073 evidence file](evidence/t073-macos-arm64.json). A pending CI result does
not establish a new Linux pass. Synthetic failure fixtures are disposable; no
user workspace or data is used by qualification.

## Additional acceptance scenarios

| Scenario | Executable evidence | Scope / observation |
|---|---|---|
| A41 | `python/tools/check_developer_core.py` no-Git variant; `crates/tf-catalog/tests/capture.rs` | Git absent from PATH; fresh default-master build and frozen source/inputs. |
| A42 | Developer-core Git variant; `crates/tf-exec/tests/branches.rs` | Attached branch lazily creates its output head; fallback reads preserve master. |
| A43 | `crates/tf-catalog/tests/git.rs`; `python/integration/test_inspection_cli.py` | Unborn/detached/broken Git and explicit branch paths fail or resolve deliberately. |
| A44 | `crates/transflow/tests/reconcile.rs`; `crates/tf-catalog/tests/git.rs`; developer-core historical/replay cases | Guards reject source/branch changes; accepted historical code does not edit current source. Scheduler composition is G2. |
| A45 | Developer-core first build; `crates/tf-catalog/tests/candidate.rs` | Forward strings register only outputs, after complete structural validation. Browse metadata uses final registered IDs. |
| A46 | Developer-core unknown/duplicate/cycle cases; `crates/tf-catalog/tests/validation.rs` | Validate/plan/build preserve registry, attempts, heads and last valid browse graph. HTTP/schedule adapters are G2. |
| A47 | `python/integration/test_catalog_cli.py`, `test_inspection_cli.py`; developer-core inspection | Validation/plan do not refresh current browse metadata or publish heads. |
| A48 | `crates/transflow/tests/reconcile.rs`; `crates/tf-exec/tests/planning.rs`; developer-core stale draft | Source/registry/head conflicts refuse acceptance; no silent replan. |
| A49 | `crates/transflow/tests/reconcile.rs` process-kill and I/O boundaries | Whole registry and exact IDs recover across journal/rename/index failures. |
| A50 | `crates/transflow/tests/reconcile.rs`; `python/integration/test_catalog_cli.py` | Explicit rename/tombstones and retained history; no inferred deletion. |
| A51 | `crates/tf-domain/tests/input.rs`; `crates/tf-exec/tests/input_resolution.rs` | All omitted/CURRENT/named and stop-fallback combinations; independent named policy. |
| A52 | `crates/tf-exec/tests/input_resolution.rs`; `python/tools/check_foreign.py` | Separate alias/version/branch/role tuples; corruption/check failure cannot select another head. |
| A53 | `python/tests/test_catalog_snapshot.py`, `test_catalog.py`, `test_discovery.py`; installed catalogue tests | Immutable C/string identities, typo rejection and workspace isolation. Native editors remain G3. |
| A54 | `crates/tf-catalog/tests/source.rs`, `compute.rs`; `python/tests/test_source_index.py`; developer-core helper edits | Nested/multiple roots and collisions; helper-only edits invalidate cache and reverting reuses exact evidence. |
| A55 | `python/tools/check_foreign.py`; `crates/transflow/tests/external.rs` | Selected/full/force consume explicit provider boundaries; provider import poison never runs, consumer certificates stay separate. |
| A56 | Foreign-data journey; `crates/tf-exec/tests/foreign.rs`, `retention.rs` | No input copies; old provider version cannot be collected while leased; renewal loss/cancel/corruption block publication; unavailable provider blocks exact replay. |
| A57 | Foreign-data journey; provider/input policy tests | Default/CURRENT/named selectors and frozen provider fallback; stop wins. |
| A58 | Developer-core and `python/tools/check_dispatch.py`; `crates/tf-plan/src/scope.rs` tests | Default/full/selected/between, side inputs, barriers/exclusions and scoped force. |
| A59 | Dispatch journey; `crates/tf-plan/src/refresh.rs` tests | Always/TTL/manual refresh matrix; force never executes foreign/imported boundaries. |
| A60 | `crates/tf-catalog/tests/traversal.rs`; installed inspection tests | Shortest-hop depth, complete wide/deep pagination, stable ordering and foreign leaves. |
| A61 | `crates/tf-exec/tests/resources.rs`, `supervision.rs`; `tools/qualification/resources/probe.py` | Real helpers with injected clock; independent hour-long defaults, explicit zero-disable and absent memory cap. No hour-long wall-clock performance claim. |
| A62 | Git-ref/replay journeys; source/reconciliation guards | G1 qualifies immutable source/output-branch primitives only. Actual scheduled requests/events remain T090–T099/G2. |
| A63 | `python/tools/check_build_commands.py`; ownership/cancellation/recovery tests | Temporary/persistent CLI, no-wait refusal, disconnect/cancel and no orphan workers. Schedule activation remains G2. |
| A64 | Public build and foreign-data journeys; `crates/tf-plan/tests/pins.rs`, `crates/tf-exec/tests/pins_replay.rs`; declaration tests | Exact pins, alias conflicts, stale drafts and original-source replay; no public Input version field. |
| A65 | Rust diagnostics/CLI tests; installed CLI and native human-output cases | Stable JSON/exit status plus actionable human explanations, malformed config/removed flags rejected. Future API/query commands remain later tasks. |
| A66 | `tools/check.sh`, spec validator and this two-repository receipt | Both roots explicitly checked; separate commit records; no runtime dependency on spec checkout. |
| A67 | Developer-core no-Git/Git clean-start journeys | Init, offline lock/sync, first build, cached selected repeat, exact rows/checks and browse metadata; no manual sync. |
| A68 | Developer-core forced output FAIL; lifecycle and dispatch journeys | Required checks remain blocking; quarantined candidate, old bytes/head and certificates survive. |
| A69 | `python/tests/test_catalog_snapshot.py`, `test_editor_overlays.py`, `test_discovery.py`; installed catalogue tests | Catalogue fingerprint tampering and cross-workspace context rejected; SDK package preserved. UI response races and native editor behavior remain G2/G3. |
| A70 | `crates/tf-catalog/tests/validation.rs`; installed catalogue tests; foreign-data/lifecycle journeys | Unknown runtime schemas/data remain deferred; structural validity never becomes data PASS. |

## Original core acceptance

| Scenarios | Executable evidence and limit |
|---|---|
| A01–A06 | Developer-core plus `check_polars_execution.py`, `check_lifecycle.py` and dispatch journeys: first registration, boundary ages, fail/warn input gating, staged output checks and one materialization. |
| A07–A11 | `check_expectations.py`, `tools/qualification/duckdb/probe.py`, shared expectation fixtures and Rust/Python semantic tests: empty data, null truth, exact keys, precision and blocking evaluator errors. Future preview/adapter portions of A10 are excluded. |
| A12–A13 | `crates/tf-catalog/tests/compute.rs`, `crates/tf-exec/tests/cache.rs`, native dispatch/helper journeys: conservative fingerprints, retained check evidence and strict corruption refusal. |
| A14–A19 | Scope/branch/input/planning/why tests and native journeys: correct write sets, fallback, exact pins, frozen Git-ref source and conflict guards. UI presentation remains G2. |
| A20–A22 | Publication/reconciliation process-kill tests plus build recovery/cancel journey: all durability boundaries, complete reservations, retry classes and explicit commit/cancel winner. Scheduling overlap is G2. |
| A23 | Source-policy tests and forced native builds prove checks and boundaries still apply. |
| A34 | `python/integration/test_catalog_cli.py`, `crates/tf-store/tests/imports.rs`, `crates/tf-exec/tests/import_publication.rs`, developer-core import journeys, local-file tests and source refresh/secret-redaction fixtures. External arbitrary APIs remain trusted user code. |
| A37 | Protocol, transport and supervision tests: bounded malformed frames, stdout separation, identity/order guards and lossless typed values. |
| A38 | G1 filesystem, terminal and log-redaction checks plus safety audit. HTTP origins, notes and browser security remain G2/G3. |
| A39 | Retention, foreign-lease and storage migration tests qualify G1 roots/schema history. Public GC/backup/restore and migration operations remain T111–T114/G3. |
| A24–A33, A35–A36, A40 | Scheduler, UI/preview/scratchpad, additional adapters, LSP, exports and release/install journeys remain later gates. Foundation probes are not delivered end-user features. |

## Failure regression findings

Ordinary builds previously left the browse graph absent or stale. Acceptance now
captures the reconciled registry, rebinds validated discovery to that capture and
checks source, configuration, Git context and owner/path identity before activating
the display pointer. The execution capture remains unchanged. Display metadata is
always labelled retained/stale until freshly validated; it cannot authorize execution.

Git-ref acceptance previously applied current-working-copy checks after its valid
fixed-source check, rejecting historical source. Fixed-source builds now verify the
retained capture and environment and never write the current registry or browse
pointer. Missing historical IDs produce the existing sync-and-commit diagnostic.
Tests execute committed helpers despite different working-tree helpers and assert
unchanged checkout/metadata, plus refusal of an unregistered committed output.

Browse persistence tests inject failure at both source guards and substitute the
cache directory before activation. The old pointer survives, operation-owned stage
files are removed, and malformed/digest-mismatched/escaping reads are rejected.
Native regressions also preserve browse metadata on invalid graphs, stale saved
plans and replay against invalid current source. Editor generation is unchanged.

Local imports previously stopped at preparation, leaving no consumable version.
The public command now uses verified artifact installation and the shared atomic
head/event transaction. Real multi-file/empty/reimport/consumer-check/pin/replay
journeys cover it. Stale head/session, active reservation and a forced late SQL
failure preserve prior heads, versions and events. This is explicit local-file
copying; the foreign-data suite separately asserts no provider-input copies.
