# Transflow

[![Toolchains & dependencies](https://github.com/jmsmistral/transflow/actions/workflows/qualification.yml/badge.svg?branch=master)](https://github.com/jmsmistral/transflow/actions/workflows/qualification.yml)
[![Rust backend & CLI](https://github.com/jmsmistral/transflow/actions/workflows/rust.yml/badge.svg?branch=master)](https://github.com/jmsmistral/transflow/actions/workflows/rust.yml)
[![Python](https://github.com/jmsmistral/transflow/actions/workflows/python.yml/badge.svg?branch=master)](https://github.com/jmsmistral/transflow/actions/workflows/python.yml)
[![Lineage web UI](https://github.com/jmsmistral/transflow/actions/workflows/web.yml/badge.svg?branch=master)](https://github.com/jmsmistral/transflow/actions/workflows/web.yml)
[![Security & license checks](https://github.com/jmsmistral/transflow/actions/workflows/safety.yml/badge.svg?branch=master)](https://github.com/jmsmistral/transflow/actions/workflows/safety.yml)

A local, code-first build system for dataframe datasets, with versioned Parquet outputs, declarative checks, and an interactive lineage interface.

**Status:** The local developer core includes public [build, history, logs, cancellation and replay](crates/transflow/BUILDS.md), explicit transient retries, restart reconciliation and a headless persistent CLI coordinator. The [customer-orders walkthrough](examples/customer-orders/README.md) exercises fresh setup, cached/forced builds, branch isolation and failed checks (T068). Environment locking includes the required DuckDB check engine. [External datasets](crates/transflow/EXTERNAL.md) support explicit registration, provider-owned resolution, leased direct reads without input replication, consumer checks, provider-dependent exact pins/replay and read-only upstream provenance (T069–T072). Foreign producer code never runs in consumer builds. Ordinary builds refresh retained catalogue browsing metadata; fixed Git-ref builds and replay preserve the current browse view. The [G1 qualification matrix](docs/development/g1-qualification.md) records core coverage and later-gate exclusions. The authenticated [loopback HTTP API](crates/tf-api/README.md) provides contextual catalogue/version/source reads, validation/diffs, plans and guarded lifecycle/build commands (T074–T076), resumable committed events and bounded exact-version previews (T077–T078). External previews read provider files directly under renewable leases. The workspace UI, scheduling, other transform adapters and release qualification remain upcoming. Earlier task evidence and outstanding prerequisites are recorded in the contributor verification history; this is not an application release.

Python **3.14 is the sole supported minor** (current qualification pin: 3.14.7).
Package installation and workspace environment checks reject other minors.

Lineage review now includes directional parent/child placement, focus-scoped Cmd/Ctrl+A, a Resource Type colour legend showing only visible node categories, and bounded metadata caching for immediate cached additions. The Align toolbar toggle optionally snaps node drags to the canvas grid; free movement is the default. The fallback-branch dialog presents an ordered view-local policy. Uncached metadata still requires a coordinator read. Existing positions and the camera stay unchanged. A newer canvas selection takes precedence over delayed catalogue selection.

Saved lineages retain node positions and membership, while pan/zoom stays transient.
Save and Save as request a name in an empty field with “Enter lineage view name…”
as its hint; the hint hides on focus, and saving requires a nonblank name.
Opening a lineage centres its node bounds at 100% zoom. The lineage picker supports
live fuzzy name search and shows each saved branch, compact version label and
last-save time in `YYYY-MM-DD HH:mm` format. New starts an empty lineage, with a
custom discard dialog protecting unsaved changes. Opening in the same effective
branch context preserves the picker and canvas until membership is ready.
Legend rows select matching visible nodes.

## Intended experience

Write Python transforms, declare their input and output datasets, and build a target without managing a separate catalogue namespace for every source directory. Transflow is designed to validate the dependency graph, resolve exact input versions, run checks, preserve the previous successful output when a build fails, and explain what needs rebuilding.

The intended architecture is a Rust coordinator/CLI with Python workers for Polars, pandas and DuckDB, SQLite metadata, immutable Parquet files, and a local browser UI. Git is supported but not required. Registered datasets from other workspaces will be consumed as read boundaries rather than automatically executing another workspace's code.

An example of the supported Polars authoring API is:

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
When the tooling interpreter was supplied explicitly, set the same absolute path in
ignored `transflow.local.toml` so CLI defaults and UI builds use it:

```toml
[python]
executable = "/path/to/tooling/python"
```

After environment setup, build a target or start the local coordinator:

```bash
# Prepare the managed Python environment and write the source modules.
transflow build curated/order_totals
transflow serve                 # authenticated API and CLI coordinator
transflow serve --open          # one-time browser connection page
```

**Initial delivery focuses on Polars transforms**, with DuckDB used internally for
validation. Pandas and DuckDB SQL transform adapters remain later work. The
[DuckDB feasibility probe](tools/qualification/duckdb/README.md) is contributor
evidence. T079 adds the private [restricted SQL helper](python/worker/SQL.md),
with a parsed AST allowlist, locked engine settings and exact-version input reads.
T080 adds the authenticated [query lifecycle API](crates/tf-api/README.md#interactive-queries):
exact local/foreign versions, independent interactive limits, cancellation and temporary
results. Foreign inputs are read directly under leases. The scratchpad UI remains
later work; queries do not publish datasets or enable a SQL transform adapter.

T081 adds [execution history and metrics APIs](crates/tf-api/README.md#execution-history-and-metrics):
separate versions, attempts and cache reuse; recorded phase timelines and critical
paths; exact medians, trailing averages and failure rates with sample counts.
Historical inspection uses captured source. Unknown timings and ETA remain explicit;
charts and schedule occurrence metrics follow in later tasks.

T083 adds the [interactive dataset graph](web/README.md#graph-exploration-t083):
keyboard-accessible exploration, depth and path queries, explicit bounded batches,
typed edges, read-only foreign boundaries and stable manual positions. Adding/removing nodes never relayouts the graph or moves the camera; the Layout control is currently a placeholder.
Compact toolbars and collapsed inspector rails leave more room for lineage.
Internal node arrows expand/retract neighbours without changing zoom. The searchable
Branch picker defaults to master; its adjacent editor sets view-local fallbacks.
Use the contributor UI bundle through `serve --ui-dir`; graph actions change only
visual exploration. T084 adds a header Save/Save as menu, named branch-following lineages, inline
single-line descriptions, revision conflicts and visual undo/redo. Groups, notes,
saved filters and fixed-source view modes are removed. JSON/SVG/PNG exports offer
label and metadata choices; legacy local
view links require the same coordinator and authentication. T085 adds the
[properties, columns and version-pinned preview inspectors](web/README.md#dataset-inspection-t085).
They show exact physical counts when metadata exists, keep missing values unknown,
show recorded creation time, and place a compact dataset row directly above the
full-width read-only grid. Headers retain the schema's letter case and unused space
beside the columns matches the blank space below the rows, including when cells
are selected. Clicking selects cells, dragging selects ranges, and the
top-left corner selects up to 1,000 displayed rows. Right-click offers a
header-inclusive copy of the existing selection, and Cmd+C or Ctrl+C copies it
from the keyboard. If the selected branch has no head,
Preview tries configured fallbacks before offering an available retained branch for browsing. Foreign
preview bytes are read directly from the provider.

A **build** contains one or more **jobs**, each responsible for one dataset.
An **attempt** is one execution try of a job; retries create further attempts.
Cached jobs reuse a version without executing a new attempt.

T086 adds numbered, syntax-highlighted read-only Code with Data publish version,
Latest saved version (the latest validated capture) and Diff, dataset jobs, bounded attempt logs and a recorded phase Build timeline
with cancellation. History opens with a duration summary; selecting a job shows its
transaction/job overview and Logs, Files, Metadata, Schema and Job spec tabs.
View build report opens a large overlay with build facts, execution-ordered jobs,
status/phase/critical-path Gantt modes and path/status filters. Active reports
refresh automatically and show live elapsed bars; completed critical paths use
retained timing evidence. The header’s Show summary button returns to the summary. Failed jobs
leave last-good Preview unchanged. History shows UTC acceptance-window metrics and
measured duration points;
reuse has no execution duration. Missing source/timings and ETA remain unavailable.
Each History entry places the dataset job duration at the right edge and shows
its overall build status and total job count,
including builds where another dataset failed. The header identifies the selected lineage branch. History dates use
`YYYY-MM-DD`; both From and To dates are included, with UTC calendar boundaries.
History and Build timestamps omit the timezone suffix while retaining UTC values.
Live timing updates preserve graph membership, positions, selection and camera;
historical versions are labelled explicitly rather than as branch fallbacks.
T088 adds status, freshness, duration, rows, files, physical size, output health and
build-role colouring with compact node labels and legends counting visible unique
nodes. Zero, Unknown and No health checks remain distinct; failed jobs do not erase
published-data availability. Duration modes use the last successful materialization
or the latest ten successful publications, excluding cache reuse. Median uses
measured samples; mean remains unknown when any selected timing is missing.
Metadata-only browsing cannot certify currentness or foreign bytes. Build
preview roles remain independent badges; schedule roles arrive with scheduling.
T089 adds Cmd/Ctrl+K catalogue search, Cmd/Ctrl+S view saving and cancelable graph
loads, with context-fenced hover/overlay refreshes. The separate dataset-list view
remains removed. A public synthetic fixture and opt-in measurement build document
observed 500-node/1,500-edge performance in the verification report.

T090 adds authenticated schedule definition storage: list/read/create and guarded
replacement of one current snapshot, with stable dataset IDs, explicit code source,
output branch and frozen fallback policies. Conflicting edits return the current
ETag. Accepted occurrence templates survive subsequent edits. Automatic dispatch
and the schedule UI are later tasks; event/evaluation foundations are described
below. See the
[schedule API](crates/tf-api/README.md#schedule-definition-storage-t090).

T091 adds internal durable scheduling event adapters. Committed dataset head,
publication, successful build and successful schedule events retain exact version
and causation evidence; provider retries and cursor replay cannot duplicate tokens.
Pending local evidence and accepted occurrence pins protect retained data from GC.
Signal-only events retain provenance without binding an input. These storage
primitives do not activate automatic scheduling or add schedule UI controls.

T092 adds deterministic AND/OR evidence evaluation and atomic occurrence queueing.
Chosen tokens, older eligible evidence for those leaves, exact input pins and
frozen build settings commit together with an audited queued request. Unchosen OR
branches keep their tokens. Paused schedules and full pending queues consume
nothing. Schema 14 adds pending-evidence indexes. Source resolution, execution and
persistent dispatch remains later work; clock generation and controls are described below.

T093 adds internal five-field cron resolution using bundled IANA timezone rules,
next-five UTC/local previews, durable intended-tick identities and clock cursors.
Sleep/restart observation follows skip, latest-per-leaf coalescing or bounded
catch-up, with missed/ignored counts. Backward clock jumps never lower the cursor.
The [clock services](crates/tf-schedule/README.md) are composed into persistent
observation by T095; the schedule editor remains later work.

T094 adds authenticated, guarded **pause/resume/run-now** API actions. Pause holds
pending automatic requests, and resume applies their frozen expiry/queue/coalescing
policies. Durable pause intervals keep delayed events and clock ticks ignored after
resume. Run-now queues an independent manual request even while paused; it does not
unpause or consume automatic tokens. Optional bounded event replay creates new
audited occurrences. Saves overwrite one definition and preserve accepted settings
and pins. See the [API guide](crates/tf-api/README.md#schedule-lifecycle-actions-t094).

T095 executes scheduled builds through the shared guarded pipeline while
`transflow serve` is running. It observes committed events and cron ticks, resolves
the saved source selector at dispatch, and preserves the occurrence's output
branch, fallback policies, parameters, retry settings and exact event input pins.
Routine clock/event observation keeps catalogue search and inspection contexts
valid; actual lifecycle, publication and phase changes still invalidate them.
Retained non-Git snapshots and clean Git refs require registered identities and
never change the foreground checkout. Mutable working-tree capture permits
additive registration only when the definition explicitly enables it. Failed
preparation records a failed occurrence without launching a job. Restart recovery
keeps the unique occurrence/build link and emits schedule success once.

This foreground backend currently dispatches serially, with conservative ancestry
and consecutive-build safeguards. Full overlap/queue/cycle policy remains T096;
the scheduling CLI/editor remain T097/T098. It does not install a background
service. See the [dispatch guide](crates/transflow/SCHEDULING.md).

T087 adds the **Build planner** inspector for the current lineage selection. With
nothing selected it shows a centered build prompt. Choose selected resources only
(the UI default), transforms connecting selected endpoints, or all eligible ancestors;
then **Next (View preview)** shows the complete scope, including hidden resources.
The preview list contains only planned build datasets, with a force toggle that recomputes the plan,
Cancel and **Run build**. Planned resources are coloured on the graph; published read
boundaries are faded. Deterministic jobs remain pending cache evaluation until their
actual inputs are resolved. The inspector keeps strategy and force controls; advanced
overrides remain available through CLI/API. Collapsed plan evidence shows read
boundaries, parameters, source policy and required checks.
**Run build** accepts exactly the reviewed plan and opens its automatically updating
Build report. Selection/context changes and expired plans automatically prepare a fresh preview;
transient conflicts retry with bounded backoff. Run build stays disabled until ready. Publishing
builds preserves node positions and camera placement. Schedule and health actions
remain later tasks.
The planned scheduling contract stores one current definition snapshot, overwritten
on Save without definition history. Edit guards prevent conflicting saves; accepted
work keeps its frozen execution settings. Scheduling implementation remains pending.
Plain-clicking any selected lineage node narrows a multi-selection to that node.
With no node selected or multiple nodes selected, the bottom tabs and right
Properties panel show a centered “Select a node to view information” notice with
an info icon. Clicking empty canvas clears the previous node’s Preview table.

The Python distribution is named **transflow**, containing both `transflow` (SDK) and `transflow_worker` (worker). See [local wheel installation and checks](python/README.md). The intended public install is `pip install transflow` after publication; no package has been published yet. The Rust binary remains a separate native artifact.

## Specification and contributor setup

Keep the canonical specification in a sibling checkout directly under `~/dev/`. These are independent repositories; the shared parent is not a Transflow project or scan root:

```text
~/dev/
  transflow/       # this implementation repository
  transflow-spec/  # authoritative design, task ledger and agent guidance
```

Start with the [task lookup guide](../transflow-spec/START_HERE.md) and [engineering agreement](../transflow-spec/AGENTS.md), then read the selected task and its relevant contracts. [AGENTS.md](AGENTS.md) is the short agent entry point; the [verification workflow](docs/development/verification.md#verification-workflow) describes staged checks and compact logs. The specification is also hosted at [jmsmistral/transflow-spec](https://github.com/jmsmistral/transflow-spec). Detailed specification files are not duplicated here.

Specification baseline: **1.1.3**. See its task ledger for intended scope; unchecked tasks are not delivered features. End users of a future installed release will not need the specification checkout.

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

Help, version, workspace initialization, explicit environment preparation,
`validate`, and catalogue sync/list/show/rename/remove are available. Polars dataset builds are available; see the [build guide](crates/transflow/BUILDS.md).
Help/version work outside either checkout without Python or Git; dataset builds
require the explicitly prepared managed Python environment.
`bash tools/check-rust.sh` runs the Rust checks without the sibling specification.

The [workspace UI shell](web/README.md) now supports authenticated branch/version
browsing, live metadata refresh and resizable panels. For a contributor build:

```bash
npm --prefix web run build
transflow --workspace /path/to/workspace serve --ui-dir "$PWD/web/dist" --open
```

Use the local `target/debug/transflow` binary when it is not installed on PATH.
Schedule/health and build-planning inspectors remain later tasks. The Vite dev
preview remains disconnected; connected browsing uses the coordinator's same-origin
session. Browser verification uses Codex’s internal Browser. Native embedded UI
packaging remains T117.

The [test infrastructure](tests/README.md) provides virtual time, deterministic
IDs/randomness, controlled failure barriers, real SQLite/filesystem fixtures and
supervised test children. It does not implement dataset publication or workers.

The initial source boundaries are [crates/](crates/README.md),
[python/](python/README.md) and [web/](web/README.md). T003 has pinned the initial
toolchain/dependency candidates and exercised them on macOS arm64. Native Linux
qualification now passes; T001–T020 are complete for their bootstrap/domain/transport/hash/diagnostic/state-machine/storage/ownership/registry scope. T009 and T011 probes pass
the native platform/Python matrix; T010 retains native editor checks for later integration.
The [catalogue overlay spike](docs/development/catalog-overlay.md) now passes runtime and
mypy checks on Python 3.14; T010 is complete for its prototype scope, with native editor checks retained for later integration.
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

Database errors now include safe numeric database/OS codes or fixed driver categories;
build draft errors also identify the failed persistence operation.
CLI options and the current JSON/error contract are documented in the
[CLI guide](crates/transflow/README.md). `--json` is available for implemented commands;
unavailable commands still fail clearly without starting work.

### Python declarations

The SDK now provides `Input`, `Output`, `Check`, `transform` and `source_transform`.
Decorated functions remain directly callable for unit tests. Declarations preserve
branch/fallback policy, immutable checks, typed parameters and source refresh
metadata. See the [authoring guide](python/DECLARATIONS.md) for examples and limits.
The [core expectation DSL](python/EXPECTATIONS.md) supports column comparisons,
null/membership/float/schema checks, row counts, primary keys and typed composition.
Structural validation and pure Rust truth/metric kernels establish exact semantics.
The [canonical evaluator](crates/tf-exec/CHECKS.md) now runs exact checks over
verified Parquet bytes in an isolated DuckDB helper. The single-attempt lifecycle
now gates execution on input checks and publication on checks of the completed
candidate. FAIL violations and evaluator errors preserve the old head; WARN data
violations remain recorded after publication. T063 adds immutable check evidence,
strict input-certificate reuse APIs and optional allowlisted failure samples capped
at 20 rows. Samples are off by default and accessible only through explicit
diagnostic service reads. The [accepted-plan dispatcher](crates/transflow/DISPATCH.md)
now composes dependency-ready execution, exact parent/pinned inputs, whole-job
cache reuse, checks and publication. It retains live materialization phases and
terminal job/build state, with bounded concurrent workers and cancellation cleanup.
T065–T067 add retry/continue policy, restart recovery and the public [build CLI](crates/transflow/BUILDS.md).

[Isolated discovery](python/DISCOVERY.md) collects declarations in fresh workers, without invoking producer functions. [Worker supervision](crates/tf-exec/SUPERVISION.md) separates authenticated control from bounded stdout/stderr logs and owns process-group cleanup. [Resource admission and phase budgets](crates/tf-exec/RESOURCES.md) provide bounded job/CPU reservations, optional estimated memory admission and independent one-hour transform/input/output validation budgets; interactive work defaults to 30 seconds. Discovery uses the workspace execution timeout, including zero-disable. Plans expose resolved resource settings and provenance; the internal dispatcher now applies these settings to dataset execution.

[Candidate reconciliation](crates/tf-catalog/CANDIDATES.md) resolves same-pass outputs and rejects graph conflicts before registry mutation.

[Registry recovery](crates/transflow/REGISTRY_RECOVERY.md) journals exact IDs before guarded file replacement and recovers interrupted indexing without rewriting user edits.

[Snapshot-bound C references](python/CATALOG.md) support explicit aliases, namespace prefixes and separate workspace test contexts without database access.

Workspace-local [catalogue editor generations](crates/tf-catalog/EDITOR.md) now have a Rust renderer and atomic refresh service (T031 implementation). Installed-SDK mypy qualification is automated; native VS Code/Emacs qualification and automatic editor integration are deferred until after the first end-to-end version.

The shared [structural validation service](crates/tf-catalog/VALIDATION.md) (T032) checks the complete captured local graph and issues context-bound certificates. Invalid submissions preserve the previous graph for display but block reuse; runtime schema/data checks remain explicitly deferred. Public validation and additive synchronization are available through the
[preparation lifecycle](crates/transflow/PREPARATION.md). Build orchestration is now exposed by the build CLI.

[Catalogue browsing and lifecycle commands](crates/transflow/CATALOG.md) provide revision-bound pages, exact retained identity lookup, rename previews/aliases and explicit guarded tombstones.

## Local Parquet imports

`dataset import <path-or-id> --path <file-or-glob>` now publishes an immutable
local version that transforms can consume. It registers an imported identity after
graph validation, validates exact Parquet schemas/values, installs durable bytes,
and atomically updates the selected branch head and publication event. Reimports
keep the dataset identity and retain earlier versions. Failed imports preserve the
last good head. Valid zero-row files are allowed; require nonempty data with an
explicit consumer expectation. `--prepare-only` retains private staging without
publishing. See the [import guide](crates/transflow/IMPORTS.md).

Local file imports deliberately copy externally mutable files into managed storage.
Registered external workspace datasets instead use leased provider reads without
copying input data. Neither path executes a provider transform during a consumer build.

The [normalization service](crates/tf-store/NORMALIZATION.md) preserves supported
null/NaN, integer/decimal precision, timestamp units/timezones and nested values,
and rejects incompatible or lossy types. The [artifact service](crates/tf-store/ARTIFACTS.md)
provides ordered multi-file artifacts, strict integrity verification and durable
installation with byte deduplication.

T038 adds the internal per-dataset publication transaction: owner fencing,
reservation/head guards, exact check linkage and durable event replay. Failed or
canceled publications preserve the last good version. Public builds use this
[publication foundation](crates/tf-store/PUBLICATION.md).

The [T039 recovery suite](docs/development/publication-recovery.md) now exercises
real process crashes around artifact installation, publication and notification,
plus disk-full/permission errors and corrupted candidates.

T040 adds exact renewable read leases and retention roots. A reader stays on its
pinned version as heads advance, while collection claims exclude new readers and
publication. See the [retention foundation](crates/tf-store/RETENTION.md); this does
not yet expose a user-facing garbage-collection command.

T041 adds output-branch selection and validated lazy creation. Read-only previews
leave missing branches absent; valid mutating preparation can create an empty
branch without copying heads or reviving a deleted name. See the
[output-branch foundation](crates/transflow/BRANCHES.md).

T042–T043 add per-input branch fallback and alias-preserving exact reads. Named
inputs retain their own policy; strict inputs never fall back. Selected heads are
leased before reading, and corrupt data or failed input checks cannot be hidden
by another branch. Repeated dataset aliases retain separate versions and roles.
These services now back public plan/build commands.


T044 adds explicit data-branch management:

```bash
cargo run --locked -- --workspace /path/to/new-analysis branch list
cargo run --locked -- --workspace /path/to/new-analysis branch create analysis
cargo run --locked -- --workspace /path/to/new-analysis branch rename analysis experiment --dry-run
cargo run --locked -- --workspace /path/to/new-analysis branch rename analysis experiment
cargo run --locked -- --workspace /path/to/new-analysis branch delete experiment --yes
```

Rename preserves branch identity. Deletion keeps history and requires `--yes`;
active uses and policy/view/schedule references block changes with an impact report.
Git and authored policy stay unchanged. See [branch lifecycle](crates/transflow/BRANCHES.md).

T045 adds internal exact-pin qualification and retained replay manifests. Historical
reads preserve the requested version and report missing original data explicitly.
Public `plan`, `build` and `build replay` commands are available; see the
[pin and replay foundation](crates/tf-store/REPLAY.md).


T046–T050 add internal full/selected/between planning, source refresh decisions,
immutable saved drafts, guarded acceptance with complete write reservations, and
conservative compute/check keys. New outputs can be planned without editing the
registry; acceptance preserves their exact proposed IDs and refuses stale context.
See the [planning service guide](crates/transflow/PLANNING.md). Public build execution now uses these services.

### Retained version reuse

T051 adds [internal branch-scoped cache services](crates/tf-store/CACHE.md).
Accepted jobs finalize keys after their parents bind, verify retained Parquet,
and reuse original versions/check evidence. Older matching versions can be adopted
through audited head changes. Force, source refresh and `cache="never"` require
execution; public build execution is now available.


### Freshness explanations

T052 adds [internal freshness and why services](crates/transflow/FRESHNESS.md).
They compare captured source and one frozen head snapshot, reporting data, logic
and ancestor staleness separately from the latest attempt and original output
quality. A failed retry can coexist with usable stale data. Missing comparison
metadata stays unknown. Public `why`/`plan` commands are exposed by T054; unassessable freshness stays unknown.


### Declared lineage traversal

T053 adds [internal upstream/downstream traversal](crates/tf-catalog/TRAVERSAL.md).
Depth is the shortest edge-hop distance, with the starting dataset at zero.
Omitted depth includes every reachable local node and registered foreign boundary.
Add `--expand-external` to `upstream` to read version-labelled provider provenance;
this imports no provider source and copies no ancestor data.
Context-bound pagination preserves unique nodes and every input alias, including
validation-only and named-branch edges. The public graph commands are available through T054.

### Plan and inspect without execution

```bash
transflow plan curated/customer_orders --json
transflow why curated/customer_orders
transflow upstream curated/customer_orders
transflow downstream raw/orders --depth 2 --json
```

`plan` saves a guarded draft with prospective writes and registrations; it calls no
producer and changes no authoring registry or data head. `why` explains the target
and reports a blocked selection without hiding available freshness evidence.
See the [inspection guide](crates/transflow/INSPECTION.md) for branch/Git context,
selection modes, exact pins, parameters, timeouts, JSON output and current limits.
See the [build guide](crates/transflow/BUILDS.md) for execution, recovery and replay.

The lineage review preview supports live fuzzy catalogue search, add-or-centre selection, drag-to-pan and Shift-drag box selection. Delete/Backspace removes selected nodes only from the focused graph view. Hover cards wait for metadata, and visible datasets display all declared connections between them.
