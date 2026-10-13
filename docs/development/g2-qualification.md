# Operational alpha qualification (T099)

Specification 1.1.3, macOS 15.3.1 arm64, Python 3.14.7, Rust 1.98.1,
Apple M4 Max / 128 GiB / APFS. This qualifies the local integrated operational
alpha, not G3 or an all-platform release. Remote CI remains owner-monitored.

## Reproduce the review

Use the [synthetic demo](../../examples/operational-alpha/README.md). The public CLI
creates four registered Polars outputs, real executed/cached history and a separate
orders publication on review. Authenticated APIs create two paused schedules and a
saved diamond view. Creation refuses existing directories. No private screenshots,
data or provider identifiers are needed. The retained review workspace is
`~/dev/transflow-workspace/t099-alpha-review`; existing older workspaces are preserved.

## Qualification matrix

| Scenarios | Real state / failure evidence |
| --- | --- |
| A24–A26 | Current Rust schedule/store suites exercise duplicate/out-of-order evidence, atomic enqueue, AND/OR remainder, DST/misfire/pause and loop bounds. The matched installed-worker dispatch runner passes 53 cases, including queued exact pins across restart, persisted missed cron, overlap and crash retry. |
| A27 | Schedule component and API/store tests cover stale ETags, frozen accepted settings and membership. Internal Browser adds simultaneous Target/Trigger roles, changes draft branch/fallback order, keeps editing, then discards; the saved definition remains unchanged. |
| A28 | API runner binds exact historical/local/foreign versions and source. Inspector regressions fence delayed selection responses, query acceptance and result identity. Browser reads real Preview/Code/query data and branch fallback; it never substitutes current source for producing capture. |
| A29 | Backend overlay tests distinguish zero, unavailable, output quality, latest failure and roles. New health tests keep published PASS separate from later input VIOLATION, escape hostile names and read provider certificates from metadata. Browser verifies actual output PASS in both health panes and light/dark layouts. |
| A30 | Native API restart retains a saved view and rejects stale edits. Browser reopens four positions at 100%, hides required build resources and reviews their authoritative scope, then restores the saved view. Dimension notifications survive controlled role/color/selection refreshes; the regression fails on the pre-fix code. |
| A31 | Component/backend metrics cover cached/retry/missing-duration populations and critical paths. Browser runs a forced four-job diamond and a paused schedule's explicit manual occurrence, then opens nested Schedule/Build reports with genuine phases/counts and completed critical-path categories. |
| A32 | Native query runner passes eight lifecycle/policy cases, including file/network/write denial, row/byte caps, timeout despite LIMIT, queued/running cancellation, zero timer, leases/tempfiles and restart. Browser runs a pinned grouping query (east 40, west 20), rejects a file query and cancels a running query to CANCELED. |
| A62 | The installed-worker dispatch runner exercises fixed non-Git capture and clean Git-ref source independent of foreground branch/dirty edits, plus invalid/unregistered source failures. Browser edits source/data branch policies without changing the saved schedule and runs its frozen source. |

## Commands and results

- `cargo test --workspace --locked --offline`: 485 unit/integration tests and one
  doctest pass. This later completion does not rewrite the earlier owner-stopped
  CI-correction gate as having passed at that earlier checkpoint.
- `bash tools/check-web.sh`: 582 tests in 26 files, strict types/lint/format,
  production worker probes and repeatable 16 local assets. Subsequent presentation
  refinements are covered by the final gate recorded in the receipt.
- `python/tools/check_api.py`: 17 real coordinator cases pass.
- `python/tools/check_queries.py`: eight real helper/coordinator cases pass.
  An initial run concurrent with other qualification I/O hit the unchanged
  30-second query deadline. A serial rerun passes; no timeout was increased and
  no universal latency guarantee is claimed.
- `python/tools/check_alpha_demo.py`: six disposable public CLI/API cases pass,
  including no overwrite, two branches, last-good preservation on forced output
  FAIL, blocked descendants and restored successful execution.
- New Python helpers pass focused Ruff formatting/lint. The unchanged matched
  SDK/worker's full Python gate (1,053 tests) and installed dispatch (53 cases)
  are qualified at implementation base `7feb246` in the immediately preceding
  interrupt-readiness delivery; new demo helpers have their separate real runner.

## Review findings addressed

SQL scratchpad and Data health were still placeholders. Both now use existing
guarded APIs, exact typed metadata and established pane styling. Recent attempt
reads use the owning accepted plan rather than the current branch context.
Branch-resolved query data has the same warning as Preview. Cancel can arrive
before a query receipt; a late accepted query is still canceled by its owned ID.
No unpublished output or fake attempt/check result is displayed.

Schedule editing also exposed React Flow nodes becoming invisible after role
refresh. Controlled nodes now retain measured dimensions separately from durable
positions. The real Browser confirms all four stay visible and keep their geometry;
the draft test is discarded and the saved view/definition are preserved.

## Boundaries

This is trusted-local-code, Polars operational alpha. SQL producers and
extended G3 checks remain separate tasks. Health's recent-detail cohort is labelled
latest 50 jobs; missing evidence stays unavailable. Queries retain at most the
backend's bounded ephemeral results and never publish datasets. Live Gantt frame
animation is covered by existing clock/component qualification; this review does
not add a new frame-rate benchmark. Linux/other-platform, editor and release gates
remain distinct. Temporary tabs and owned coordinators are closed/stopped after
review; the persistent synthetic workspace remains ready for the owner.
