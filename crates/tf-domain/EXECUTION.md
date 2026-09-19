# Execution domain (T017)

`tf_domain::execution` validates in-memory build, job and attempt transitions.
It has no clock, process, database or filesystem dependency. Callers must persist
accepted transitions before exposing them. These types do not implement a scheduler
or prove atomic publication; storage and supervision integrate them in later tasks.

## Job and attempt transitions

| Current state | Event and guard | Next state |
|---|---|---|
| PLANNED | Wait for dependencies | WAITING_DEPENDENCIES |
| PLANNED / WAITING_DEPENDENCIES | Queue, without cancellation | QUEUED |
| PLANNED / WAITING_DEPENDENCIES / QUEUED | Reuse an existing version, without cancellation | CACHED; no attempt |
| PLANNED / WAITING_DEPENDENCIES / QUEUED | Same-build/plan unsuccessful upstream terminal evidence | BLOCKED; no attempt |
| QUEUED / RETRY_WAIT | Fresh attempt ID, available budget, elapsed delay, no cancellation | STARTING; new real interval |
| STARTING | Advance | VALIDATING_INPUTS |
| VALIDATING_INPUTS | Advance | RUNNING |
| RUNNING | Advance | MATERIALIZING |
| MATERIALIZING | Advance | VALIDATING_OUTPUTS |
| VALIDATING_OUTPUTS | Seal candidate intent, without cancellation | COMMITTING |
| COMMITTING | Exact intent, attempt, owner fence and head generation; no cancellation | SUCCEEDED |
| Any execution phase | Record failure | FAILED attempt; RETRY_WAIT or FAILED job |
| Any nonterminal job | Acknowledge requested cancellation and cleanup | CANCELED; closes only an active attempt |
| Any nonterminal job | Established replacement ownership | INTERRUPTED; closes only an active attempt |
| Any terminal job | Any mutation | Rejected; evidence unchanged |

Every attempt event checks its ID and exact coordinator-session/reservation fence.
Phase advancement cannot skip validation/materialization, repeat a phase, or enter
COMMITTING without candidate evidence. Rejected events do not partially mutate state.
Terminal attempts remain unchanged across retries. Cached and blocked jobs have no
attempt, execution timestamps or materialization; cached results reference an existing
version whose original evidence must be loaded by the caller.

## Retries, cancellation and publication

The default policy permits one attempt. Additional attempts require an explicit
budget and approved transient classes (`WorkerUnavailable`, `TransientIo`). Input or
output violations, invalid schemas, import/syntax failures, unsupported operations,
publication conflicts and unknown failures cannot be configured for automatic retry.
Delay is 1, 2, 4, 8, 16 then 30 seconds, capped at 30, with no jitter. A retry starts
with a fresh ID and preserves the accepted plan/source binding. The future immutable
plan owns exact parameters and input versions; this module does not resolve them.

Cancellation is an idempotent request, separate from process cleanup acknowledgement.
It blocks phase advancement, retries and publication. A recorded execution failure
can still finish as FAILED after a request; cleanup acknowledgement finishes as
CANCELED. A request after successful publication returns `TooLate` and preserves
success. Build requests must be propagated to jobs by future supervision.

Candidate intent binds build, job, accepted plan/source, attempt, destination,
expected head generation, version UUID, artifact digest and owner fence. A new try
cannot reuse a candidate version already used in that job. Identical artifact bytes
may belong to distinct publication UUIDs. Global UUID uniqueness, hash verification,
required check results and reservation ownership are caller/storage obligations.
Commit validates the exact sealed intent and checked head-generation increment.
Storage must perform these guards and persist the success transition in one visibility
transaction; an in-memory success alone is not proof that files or a DB were committed.
Recovery accepts a different session or higher same-session generation only after the
caller has established actual ownership. Superseded attempts cannot publish through
this state object. Cross-process fencing remains storage/coordinator work.

## Build transitions and evidence

| Current state | Event and guard | Next state |
|---|---|---|
| QUEUED | Start, without cancellation | RUNNING |
| QUEUED | Finish exact terminal job set, with no real attempts | Derived terminal outcome |
| RUNNING | Finish exact terminal job set | Derived terminal outcome |
| Any terminal state | Start/finish | Rejected; state unchanged |

The complete accepted job set must have unique IDs and at least one mandatory job.
Finish requires every listed job, including optional work, to be terminal and to match
the build and plan/source binding. Missing, duplicate, foreign or still-running work
is rejected. Required FAILED/BLOCKED takes precedence over INTERRUPTED, then CANCELED;
otherwise the build succeeds. Optional failures do not invalidate required successful
or cached results. A late cancellation request cannot rewrite committed results.
Job/attempt evidence stays with those objects; build aggregation does not copy it.

`EventTime` is caller-supplied milliseconds on one timeline, nondecreasing per entity.
Blocking cannot precede upstream terminal evidence; build completion cannot precede
any job's latest event. Clock reconciliation and durable timestamp conversion belong
to the caller. Retry and head-counter overflow are rejected before mutation.

## Verification and limits

`cargo test -p tf-domain --test execution --locked --offline` runs 12 tests, including
all phase pairs, failed-then-successful retries, cancellation/publication orderings,
stale fences, cross-job candidates, exact build membership and overflow rejection.
Seeded property histories exercise 57,344 event steps and assert unchanged state on
errors, immutable completed attempts and absence of fabricated cached/blocked intervals.
No property-testing dependency is introduced.

These are A20/A22 domain prerequisites. Actual SQLite/filesystem crash recovery,
process exit acknowledgement, durable journals, worker retries and publication races
remain T018/T019/T037–T039/T055/T057/T065/T066. Runtime restoration codecs and API
projections are deliberately outside this pure state model.
