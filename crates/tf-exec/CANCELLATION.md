# Build cancellation (T057)

`cancellation::BuildControl` connects the durable cancellation/publication winner
with owned worker cleanup. The coordinator retains one control per accepted build,
created from its real `RuntimeOwner` and an explicit active-attempt limit
(1–10,000). Waiting clients observe this control; they do not own it or its worker
futures. Metadata-only owners cannot use it. After supervisor cleanup and durable
attempt completion, `finished` releases its capability slot; retries retain bounded
active controls while SQLite retains immutable attempt identities.

Before spawn, `register` checks the exact workspace, build, attempt, coordinator
session and fence against the accepted plan, live job/attempt and write reservation.
Canceled, terminal, committing, missing and stale attempts are rejected. A worker
capability checks the launch request's attempt UUID. Clones share the same
cancellation token for sequential or admitted concurrent validation helpers. Duplicate
registration cannot retarget an existing token. Each supervisor independently
creates its private random channel nonce and verifies every frame's request/attempt
identity. This service retains only bounded cancellation tokens, not log buffers
or raw process handles. Helper clones retain the same attempt identity; each launch still requires its
resolved admission/timer policy and receives a fresh authenticated channel.

`request` runs the existing short SQLite cancellation transaction first. Only after
that commit returns does it set the registered workers' cancellation tokens. A
storage/authority failure does not signal workers. The persisted request prevents
later publication even while TERM-resistant processes are still stopping, and also
prevents new registrations. A registered but not yet launched worker sees the token
and never spawns. Repeat requests return `AlreadyRequested`; an interrupted database
acknowledgement must be retried/read back rather than assumed to have failed.

The visibility transaction is the arbiter. Cancellation before publication retains
the old head and leaves installed orphan objects invisible. Publication first keeps
the successful attempt/job, version, provenance, checks, event and outbox; if no live
jobs remain, cancellation returns `TooLate`. With unfinished siblings, those jobs
can still be canceled without undoing an earlier publication. A request is not
cleanup acknowledgement: it neither marks jobs/builds terminal nor releases write
reservations. Existing domain completion rules and terminal reservation release
remain responsible for those transitions.

| Client event | Persistent coordinator | Temporary coordinator |
|---|---|---|
| Waiting connection disconnects | Continue owned work | Persist cancellation and request cleanup |
| Explicit Ctrl-C/cancel for this build | Persist cancellation and request cleanup | Same |

`client_event` implements this distinction using the actual owner's mode. The
coordinator owns `Worker::run_async` independently of its waiting CLI, and awaits
worker completion before transient shutdown. Dropping that coordinator future or
`BuildControl` requests emergency cleanup; this is not a durable user cancellation
receipt. An unstarted capability is canceled too. Recovery must reconcile unfinished
state after coordinator loss. A normal persistent observer disconnect does neither.
The existing no-wait policy still requires persistent ownership before mutation.

## Process identity and signal authority

The supervisor captures the OS start record immediately after spawning a fresh
owned group and returns it with the diagnostic PID in `Report`. Before each group
signal it refuses reaped handles, missing/mismatched start records, or a child that
is no longer waitable. Linux identity combines boot ID and process start ticks;
macOS uses fixed-locale `ps` start time. The macOS record has second precision, so
it cannot authorize a signal alone.

`waitid(WNOWAIT)` retains the direct child's identity until the final group signal,
closing the check-to-signal PID-reuse race. No caller can supply a saved PID/start
pair for signaling. Cleanup uses TERM, the configured grace (five seconds by
default), KILL and direct-child reaping; the direct owned child has a kill fallback.
Start-record refusal never authorizes a group signal, including in emergency Drop.
The direct-child fallback remains safe because the unreaped owned handle pins its
PID. Trusted code can escape groups; this is not hostile-code containment.

After actual lock reacquisition, existing publication recovery establishes a new
session, interrupts unfinished work and releases stale reservations. Old controls,
registrations and publication intents fail their fences. Persisted PID/start
records alone are insufficient for safe orphan signaling. T066 adds private worker
capabilities: a mutually authenticated live worker can terminate its own group
after the new owner fences the old session. No saved-PID signal is issued. Failed
authentication or unavailable cleanup for a still-live worker fails closed. T067
adds authenticated CLI routing and cleanup-aware Ctrl-C; see the
[build/recovery guide](../transflow/BUILDS.md). HTTP/browser control remains T075.

## Verification

Native tests use fresh isolated child groups only. A stale start-record fixture
refuses TERM and KILL while its canary stays alive; a reaped handle cannot signal.
Disconnect tests stop TERM-resistant descendants and reap the leader while another
build continues. Persistent disconnect leaves work running until explicit cancel.
Database refusal leaves the live worker untouched; losing its owning control then
cleans it up without fabricating a persisted receipt.

Real SQLite tests cover both deterministic transaction orderings, cancellation
after object installation, precanceled launch, repeated requests, exact identity
and capacity checks, lock/session replacement and two simultaneous writer
transactions. Existing recovery tests cover publication crash boundaries, partial
build success, disjoint/overlapping reservations and immutable event replay.
[Local evidence](../../docs/development/evidence/t057-macos-arm64.json) distinguishes
executed macOS checks from CI handed to the owner.
