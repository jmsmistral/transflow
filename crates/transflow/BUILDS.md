# Building, inspecting and replaying datasets

After explicit environment lock/sync, `transflow build curated/orders` captures and
validates the complete graph, reconciles new output declarations, plans the selected
scope, executes required checks and waits for publication. No separate trust,
discovery or catalogue-sync command is required. The initial execution adapter is
Polars; failed checks or attempts preserve the last successful head.

```bash
transflow build curated/orders --branch development
transflow build curated/orders --force --json
transflow plan curated/orders --branch development --json
transflow build --plan <plan-id>
transflow build list --limit 20
transflow build show <build-id> --json
transflow build logs <build-id> --follow
transflow build cancel <build-id>
transflow build replay <build-id> --branch replay-review
```

Build accepts the same scope, boundary, exclusion, fallback, pin, parameter, source
and resource-selection flags as `plan`. `--force` bypasses whole-job reuse. A saved
draft is accepted exactly once with its original guards; selection overrides,
expired drafts and changed source/catalogue/heads fail rather than silently replan.
`--plan` needs only the draft ID: it uses the captured environment selection.

Human builds stream job transitions to stderr and print their terminal identity
and outcome to stdout. `--json` emits one terminal `CliEnvelopeV1` object to stdout,
with no progress output. Its execution result includes the accepted plan, source,
exact input versions, jobs, attempts, phase timings, process reports and checks.
Cached jobs refer to original validation evidence and timestamps. Samples and
private recovery capabilities are excluded. Failed builds exit 1; canceled waiting
builds exit 130; invalid syntax exits 2. A successful WARN violation is visible in
both the retained checks and the human summary.

`build list` is bounded to 1–100 entries and uses a stable ID cursor. `show` returns
a consistent snapshot. Logs are separate bounded stdout/stderr files; inspection
rejects symlink substitutions and caps a combined response at 16 MiB with an explicit
truncation indicator. `--follow` streams human logs until terminal state or the cap;
JSON mode returns the collected chunks once. Stopping a log observer does not cancel
its build. User-authored logs can contain sensitive data; this is not a secrecy guarantee.

## Persistent and temporary ownership

Waiting builds acquire a temporary coordinator when none is running. Ctrl-C requests
durable cancellation of the active build and waits for process cleanup. To submit
work that survives its client exiting, run a foreground headless coordinator in a
separate terminal:

```bash
transflow serve
transflow build curated/orders --no-wait
```

`--no-wait` checks for a live persistent owner before discovery or plan mutation and
returns an accepted build identity. Waiting clients also route to that owner.
Control uses a bounded private authenticated loopback protocol, not a browser API.
Only one build executes at a time per coordinator; a concurrent submission is
explicitly refused. Independent jobs within a build still run concurrently under
the configured resource pool. Ctrl-C on `serve` cancels active work and stops it.
There is no automatic daemon installation. HTTP/UI, `serve --open` and scheduling
are later tasks. Standalone `plan`/catalogue mutations currently require the owner
to be stopped; build submissions perform their own planning under the owner.

## Retry and restart policy

Workspace configuration can opt into retries and independent-branch continuation:

```toml
[execution]
max_attempts = 3
retryable_classes = ["transient_io", "worker_unavailable"]
abort_on_failure = false
```

Defaults are one total attempt, no retryable classes and abort-on-failure enabled.
The limit includes the first attempt and is capped at 100. Backoff is 1 second,
doubling up to 30 seconds without jitter. Each retry gets a fresh attempt and staging
area but retains identical source, parameters and exact inputs, including held input
leases. Earlier failures remain visible. `transflow.TransientIOError` is the explicit
source-adapter signal for a safe transient I/O retry; adapters must separately bound
any HTTP retries. Generic exceptions, evaluator errors, data violations, invalid
schemas/references/imports and publication conflicts are never inferred transient.
`worker_unavailable` covers a transient OS resource-unavailable launch refusal.

A terminal parent failure blocks descendants. Abort policy cancels independent
active work; continue policy lets it finish. Committed versions remain valid.
An explicit memory budget conservatively reserves the whole configured budget per
public CLI worker; it is admission accounting, not a measured or enforced RSS cap.
No memory ceiling is introduced by default.

On restart, the new owner fences the old session, preserves committed publications
and their undelivered notifications, interrupts unfinished attempts and releases
stale reservations/leases. It validates private worker capabilities before emergency
self-group termination; it never signals a persisted PID. If authenticated cleanup
cannot be established for a still-live worker, recovery refuses further execution.
Registry journals reconcile with their existing guarded protocol. Source captures,
failed candidates and diagnostics are retained. Untouched accepted queues resume
with their original plan; queued cancellation starts no producer. Interrupted code
is not automatically retried: interruption is not an enabled transient class.

Replay creates a new forced build using retained original source/environment,
parameters, selection and boundary versions on the requested destination branch.
Planned parents execute again within that same historical scope. It does not read a
new checkout or substitute latest inputs. Missing retained inputs/environment or
conflicting destination guards fail explicitly. This cannot guarantee deterministic
hidden external I/O; secret delivery remains separate work. Registered foreign inputs
use verified local replicas and retain exact origin identity for offline pin/replay;
fresh unpinned builds still require provider resolution. See [external data](EXTERNAL.md).
