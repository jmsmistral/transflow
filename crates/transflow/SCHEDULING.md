# Persistent scheduled dispatch

Start the foreground coordinator with `transflow --workspace <path> serve`.
While it is running, saved unpaused schedules observe committed events and cron
ticks. The authenticated schedule API can also queue an independent manual request
while paused. Stopping the coordinator stops observation; restarting applies the
saved missed-tick policy and reconciles accepted builds before dispatching new work.
The [schedule guide](SCHEDULES.md) documents the CLI and lineage editor.

The dispatch path uses the ordinary source discovery, structural validation,
planning, reservation, installed-worker execution, checks and publication services.
It resolves the saved source selector when dispatching each accepted occurrence:

- A clean Git ref captures the selected committed tree without switching the live
  checkout. Its registry must already contain all producer identities.
- A fixed snapshot revalidates retained source, including non-Git source. It checks
  captured dependency files against the explicitly prepared installed environment.
- Working-tree capture is deliberate. Additive registration is denied unless
  `allow_additive_sync` is true; permitted registration retains the normal guards.

Output branch, local/provider fallback rules, parameters, force/currentness,
timeouts and retry/abort settings come from the frozen accepted request. Event
payload pins bind exact local or foreign versions, even when their branch differs
from the output branch. Named input branches must match the pin's branch; conflicting
or unmatched pins fail. Pinned inputs cannot also be requested build outputs.
Provider files remain in the provider workspace under renewable leases. No provider
producer is executed and no provider dataset bytes are replicated locally.

Acceptance commits reservations, the build and both occurrence/build links in one
SQLite transaction. Failure before acceptance leaves no executable job. Terminal
reconciliation records failure/cancellation, or commits one schedule-success event.
An interrupted accepted build is not submitted as a second build on restart.

Observation processes one ID-keyset page (up to 100 schedules/1 MiB), 100 committed
events per schedule and the existing bounded clock pass. It delivers one intended
tick per leaf before evaluation, preserving separate catch-up occurrences. A pass
reconciles up to 100 terminal links and visits at most ten queued candidates from
a bounded page of 100. Disjoint work can be admitted while other workers compute. Subsequent
passes continue the keyset; no schedules are silently dropped. Unresolvable clock
definitions are marked for review without stopping unrelated schedules.

One persistent OS authority delegates execution handles while serializing short
metadata writes. Global admission and complete write reservations permit disjoint
builds to run concurrently. Conflicting automatic occurrences wait for reservations,
without inventing failed jobs. Startup recovery runs once per shared owning session.

Frozen policy controls same-schedule overlap and bounded pending coalescing, queues
or skips. Compatible queued templates may be replaced atomically; manual/held/active
work and distinct templates remain independent. Static authored cycles, causal
ancestry, a 16-hop bound, minimum delay and bounded new-evidence bursts prevent
automatic loops. Manual requests bypass the runtime retrigger checks. Temporary CLI
and metadata-only owners do not activate scheduling. Observation and authenticated
control continue during worker computation; queued user commands retain priority.

See the [schedule API](../tf-api/README.md#schedule-lifecycle-actions-t094) and
[local qualification receipt](../../docs/development/evidence/t096-t098-macos-arm64.json).
