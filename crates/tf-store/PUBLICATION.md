# Per-dataset publication (T038)

The `tf-exec::publication` service holds the real runtime owner throughout blocking
filesystem and SQLite work. Dropping the caller's future does not drop that owner
while its blocking task continues. Metadata-only owners cannot recover or publish.
Startup first registers the workspace and acquires its OS lock, then calls
`recover`; a supplied UUID alone is never authorization to become coordinator.

The internal coordinator integration sequence is:

1. Persist the accepted plan, build/jobs and attempts through execution services.
   Reserve the complete build write set, including expected head generations.
2. Freeze each job's exact historical input provenance and complete check contract
   before execution leaves STARTING. This is a trusted planner operation, not a
   worker message. The contract is immutable across retries. Check definitions
   retain input/output phase, alias and FAIL/WARN severity.
3. Materialize once and evaluate the exact candidate. The caller supplies the
   domain's `PublicationIntent` and the frozen contract. Every required result must
   exist, belong to this attempt and certify the exact output digest or input
   alias/version/digest. Missing/skipped/ERROR results block publication; WARN can
   permit a data violation only. Unexpected extra results also fail closed.
4. The service independently copies/verifies those bytes, writes the durable
   intent, then installs and flushes the immutable object. An artifacts row at this
   stage records manifest evidence only; visibility requires a published version.
5. One IMMEDIATE SQLite transaction rechecks the runtime session, reservation
   fence, accepted plan/source, attempt/job state, cancellation, frozen evidence,
   live dataset/branch and expected generation. It writes version/input/check
   links, CAS head, terminal success, head-change event and outbox together.
6. Notify only from the persisted outbox. Repeated reads return the same event ID;
   acknowledgement follows delivery, so a crash can repeat delivery, not execution.
   Reservations remain until the build reaches its terminal state.

Physical byte deduplication and version identity remain separate. A forced new
publication can share an artifact but gets its own UUID, attempt, provenance,
event and generation. Atomicity is per dataset. A later cancellation/failure does
not undo successful versions. Cancellation first blocks commit; committed jobs
return TooLate for a subsequent cancellation request. Pending siblings can still
be canceled. A generation conflict requires a new plan, never an implicit rebase.

SQLite schema 3 adds frozen contracts, expected reservation generations and
immutable published check-result links. Existing migrations are unchanged.
Original result timestamps are retained; linking checks does not reevaluate them.
Repository transaction methods contain no filesystem/hash/network/user callbacks.
Artifact work is synchronous only within a blocking task. Public Store methods
are internal trusted coordinator primitives whose documented ownership precondition
must be upheld; no SQL connection or transaction is exposed to workers.

After actual new OS ownership, recovery fences the old session, abandons prepared
intents, interrupts nonterminal attempts/jobs/builds, and releases old reservations.
Committed versions/success/events survive. Orphans and interrupted staging are
retained, invisible, for later retention/GC work. Worker process-group cleanup is
still T057/T066; this storage recovery does not claim to have killed old workers.

These are backend foundations. Accepted-plan composition, worker execution and
canonical check evaluation remain T049/T055–T064. There is no public `build` or
import-publication command yet. The test fixture seeds those future planner/worker
records explicitly, then runs the actual artifact/publication/ownership services.
Recovery tests prove process-crash behavior on actual local filesystems, not power
loss, malicious owner writes or filesystem/device flush reliability.

[T039 recovery qualification](../../docs/development/publication-recovery.md)
now kills real publication subprocesses at nine boundaries and checks persisted
heads, immutable objects, interrupted/successful attempts and exact event replay.
