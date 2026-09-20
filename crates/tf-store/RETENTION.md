# Read leases and retention roots (T040)

`Store::acquire_read` reads a head and pins its exact version in one IMMEDIATE
transaction. Query, preview-plan, build and provider-copy leases use the same
mechanism. Renewal extends that original binding, never follows a newer head.
Lease tickets contain an operation identity and generation: old tickets cannot
renew/release a newer generation. Released IDs remain recorded to prevent reuse.
Expired leases cannot be revived. Provider metadata-only owners can serve leases
without importing code, activating schedules or publishing datasets.

Every timed operation takes an explicit UTC microsecond sample. A persisted
nondecreasing clock floor survives restart and even refused requests. Clock
rollback fails visibly rather than resurrecting expired protection. Tests advance
this clock without sleeps. Lease expiry and draft validity are independent: a
renewed lease does not make an outdated plan acceptable.

Root snapshots include live heads; the union of latest 20 versions per branch and
30 days by default; explicit history/plan/schedule/backup/replica pins; live leases;
active build inputs, frozen input contracts and prepared candidates. Local version
inputs are followed transitively; foreign input artifact identities protect local
replicas without pinning the provider's entire history. Retained/active source
captures are returned too. Snapshots are capped at 100,000 identities per category;
over-limit snapshots fail rather than silently omit roots.

`claim_collection` atomically rechecks roots and creates an exclusive artifact
claim. New leases, explicit pins, build input acceptance and publication reject a
claimed object; new retained lineage cannot cross an existing collection claim.
The claim does not automatically release on drop/crash. Abandon it only if bytes
have not been deleted. Runtime ownership serializes all callers; these repository
methods are internal primitives, not worker or unauthenticated client APIs.

No file is deleted by this module. The later GC service (T112) must enforce
orphan/quarantine grace periods, dry-run/explicit approval, filesystem deletion
and interrupted-claim reconciliation. Provider copy/export, planning and full
worker/query integration remain later tasks. A lease protects identity/lifetime,
not integrity: readers must still strictly verify their pinned artifact before use.
