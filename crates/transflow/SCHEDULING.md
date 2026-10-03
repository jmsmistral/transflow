# Persistent scheduled dispatch (T095)

Start the foreground coordinator with `transflow --workspace <path> serve`.
While it is running, saved unpaused schedules observe committed events and cron
ticks. The authenticated schedule API can also queue an independent manual request
while paused. Stopping the coordinator stops observation; restarting applies the
saved missed-tick policy and reconciles accepted builds before dispatching new work.
The scheduling CLI and editor are later tasks.

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
reconciles up to 100 terminal links and accepts at most one new build. Subsequent
passes continue the keyset; no schedules are silently dropped. Unresolvable clock
definitions are marked for review without stopping unrelated schedules.

Dispatch is conservatively serial until T096 implements complete overlap and
queue policies. Automatic work respects the frozen minimum delay and rejects
repeated schedule ancestry or the consecutive-build limit. Manual requests bypass
those retrigger checks. Full static cycle analysis, active coalescing/queue/skip
and overlapping-build policy remain T096. Temporary CLI and metadata-only owners
do not activate scheduling. Idle observation does not advertise API busy status;
already queued user commands take priority over new scheduled preparation.

See the [schedule API](../tf-api/README.md#schedule-lifecycle-actions-t094) and
[local qualification receipt](../../docs/development/evidence/t095-macos-arm64.json).
