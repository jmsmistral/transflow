# Accepted-plan execution (T064–T067)

`dispatch::run` is an internal application service. It consumes an untouched
accepted build and retains `RuntimeOwner` until the workers have stopped. The public [build CLI](BUILDS.md) composes planning, retry/continue policy and restart
reconciliation around this service.

Before starting any producer, the service reopens the accepted source capture,
verifies the exact managed environment, validates the complete captured graph,
checks all boundary identities/bytes and renews the accepted leases. It executes
only the accepted write set. Planned inputs wait for that build's successful or
cached parent result; boundary inputs retain the exact accepted versions, including
historical pins. Neither path looks up a convenient replacement head.

A single coordinator loop serializes SQLite mutations. Ready independent jobs run
in scoped worker threads under one admission pool. Each job holds its reservation
through input checks, transform, streaming sink, output checks and publication.
Helpers reuse the same reservation, with separately timed validation phases.
Configured memory admission requires caller-supplied positive per-job estimates via
`run_with_options`; missing/oversized estimates refuse before a producer starts.
The public CLI conservatively supplies the entire configured budget for each job.
These estimates are not hard producer memory limits. Canonical check helpers use
an explicit 1 GiB spill limit and the optional estimated memory allocation. Without
a memory budget, no default memory cap is introduced.

The final compute/check contract is frozen after all parents resolve. Exact
`job_inputs` records survive both execution and cache reuse. Whole-job reuse uses
the shared branch-scoped lookup, integrity verification, original check evidence
and atomic adoption services; cached jobs create no attempt. Forced jobs and
refreshing sources execute. Jobs that execute evaluate every declared check;
individual input-certificate lookup remains an optional future optimization.

The coordinator persists STARTING, VALIDATING_INPUTS, RUNNING, MATERIALIZING,
VALIDATING_OUTPUTS and COMMITTING independently. A bounded nonblocking supervisor
channel reports the actual sink transition with its monotonic observation time;
queue overflow fails the worker rather than blocking pipe drains. Publication's
intent callback records COMMITTING outside the intent transaction. Timings close
on terminal outcomes. Complete check results and private samples use the existing
evidence service; ordinary reports contain process/log metadata and sample-free
failures, not row payloads.

Publication is the existing guarded candidate-install/visibility transaction.
Canceled/failed attempts cannot replace the old head. A terminal failure blocks
planned descendants and requests durable cancellation of independent work; the default policy uses one attempt and abort-on-failure. Explicit transient
classes can opt into bounded retries; continue policy permits independent branches.
Retry contracts and leases remain frozen, and prior attempts remain immutable. Explicit cancellation
and dropped coordinator futures stop owned workers; servers must own the future
independently of client connections. Reservations/leases are released only after
cleanup and terminal aggregation. Preflight refusals cancel the untouched accepted
jobs and release their reservations. Unexpected coordinator/storage failure retains
unfinished state for recovery rather than inventing a terminal success.

The initial execution adapter is Polars with normalized Parquet, zstd, 8192-row
writer groups and a stable zero random seed. The logical evaluation time is frozen
at plan creation. Until discovery explicitly reports context usage, any captured
file containing `ctx` conservatively includes that clock in computation keys.
Declared secret delivery is not yet available and refuses during preflight.
Source/environment/parameters/check policies stay bound to the accepted plan.

`python/tools/check_dispatch.py` exercises packaged workers in a real managed,
hash-locked environment. `tools/qualification/check.sh` runs it after the other
engine probes. Explicit setup must populate `target/qualification/wheelhouse`:

```bash
python -m pip download --require-hashes --only-binary=:all: \
  -r tools/qualification/python/py314.lock --dest target/qualification/wheelhouse
```

The qualification workflow performs that setup before its offline tests. Worker
socket/process tests need normal local process permissions. Run the contributor
aggregate and native qualification sequentially to avoid overlapping their worker
startup tests.
