# SQLite foundation (T018)

`Store` owns one private, non-cloneable SQLx connection. Mutating repository methods
require an exclusive borrow; the coordinator's mutation loop owns this writer.
Transactions are fixed repository operations with no caller callback, worker wait,
network request or artifact scan. Future coordinator dispatch may add a bounded
queue around this interface. Runtime ownership is a separate T019 prerequisite.

The linked bundled SQLite is queried directly and must include the WAL-reset fix
(version 3.51.3 or later). Open verifies WAL, foreign keys, synchronous FULL and a
250 ms busy timeout. `StorageInfo` retains actual version/source and pragma evidence.
No system SQLite upgrade is needed. See [SQLite WAL documentation](https://sqlite.org/wal.html).

Filesystem admission uses safe `rustix` statfs and allows APFS/HFS on macOS and
ext4/XFS/Btrfs on Linux. Unknown, network, FUSE and temporary-memory filesystem types
are refused; type admission alone does not establish power-loss durability for every
mount/device. A database file cannot be a symlink. The directory must already exist;
workspace initialization and robust ownership/path protection remain separate tasks.

## Schema and migrations

Thirteen forward migrations install 53 logical tables: source/catalogue projections,
versions/heads, builds/jobs/attempts, checks, scheduling, events/outbox, pins/audit,
registry mutation journals, foreign replicas/leases, frozen publication/check
links, read retention, replay, cache associations and computation comparison evidence.
Schedule definition storage is implemented; scheduling evaluation/dispatch remain later work.
External data is consumed from its provider under leases.

The database has an application ID, `user_version` and monotonic checksum ledger.
Migration checksums are SHA-256 of exact embedded SQL bytes. Open rejects unrelated
or newer databases, missing/reordered ledger entries and changed checksums. All
pending DDL and version markers commit in one IMMEDIATE transaction; failure rolls
back the whole upgrade. Existing migration bytes must never be edited after delivery.
Migration 13 replaces the pre-release schedule-revision placeholders with one current
definition. Before upgrading a store with schedule rows, SQLite `VACUUM INTO`
creates a private `schedule-schema-<old-version>-backup-<nonce>.sqlite` beside the
runtime database, including committed WAL content. Backup failure aborts opening;
backups are never overwritten or automatically removed. Retain this private backup
until the upgrade is accepted; restore it with a compatible older binary if needed.
The migration preserves accepted execution settings and consumed token evidence;
legacy current definitions require review and an explicit normalized save. Other
destructive upgrades need their own backup policy; there is no downgrade.

STRICT tables, foreign keys, compound head/version identity, unique input aliases,
attempt numbers and occurrence evidence constrain writes. Immutable snapshots,
versions and declarations reject updates; terminal attempts cannot be rewritten.
Events, head changes and audit evidence are append-only. Head generations must
increase. A version's producing attempt must belong to that dataset. The publication service adds atomic head/event/version transitions and session,
reservation, evidence, cancellation and generation guards.

Each store indexes one workspace; its singleton constraint allows local dataset IDs
to be primary keys. Foreign versions use the complete workspace/dataset/version tuple,
so identical UUID bytes in another workspace remain distinct. Provenance references
to foreign identities do not imply local dataset ownership. UTC timestamps are signed
microseconds (`*_at_us`); measured durations are separate monotonic nanoseconds.

## Repository scope

Current APIs register existing workspace/dataset identities and append a bounded
event plus audit row atomically. They never allocate IDs. Read-only repositories
return owned results, capped at 100 events per page, and expose no open cursor or
transaction. Payloads are bounded to 1 MiB before opening a write transaction.
Callers must provide sanitized evidence; arbitrary JSON is not a secret scrubber.
Readers should be closed promptly. Connection errors retain a technical source but
display a fixed summary. No generic SQL/transaction escape hatch is public.

Explicit `close().await` completes shutdown. A dropped SQLx transaction queues its
rollback before the connection can service another operation. Caller cancellation
after a commit starts may have an uncertain result; read the durable identity/event
before retrying. The publication service serializes cancellation against its visibility commit.

The initial nine real-file tests cover fresh/reopen/upgrade, compatibility/checksum refusal,
foreign keys and identity constraints, injected migration/audit failure, bounded
contention, concurrent WAL snapshots, immutable evidence and symlink refusal.
Publication crash recovery is now qualified separately under T039; backup/restore
and public build composition remain later tasks.

[Logical normalization](NORMALIZATION.md) projects Arrow/Parquet schemas, validates
actual values, emits bounded typed cells and exposes per-adapter schema guards.
The import service retains this evidence without publishing artifacts or heads.

[Artifact storage](ARTIFACTS.md) supplies private staging, canonical ordered
manifests, strict Parquet/hash verification and immutable no-replace installation.
It does not expose a head or version before the publication service commits one.

[Per-dataset publication](PUBLICATION.md) now supplies guarded visibility,
immutable provenance/check linkage and persisted outbox replay. Schema 3 adds two
publication tables (42 logical tables plus the migration ledger). The coordinator execution
lifecycle and public build composition remain subsequent tasks.

[Read leases and retention](RETENTION.md) add schema 4: a persisted retention clock
and exclusive collection claims, plus lease kind/release state. Root snapshots
cover history, leases/pins, active builds and transitive local/foreign provenance.
There are now 44 logical tables plus the migration ledger. Physical GC is later work.

Output branch previews are available on the read-only `Reader`. Guarded lazy
creation atomically records an empty branch and audit evidence; the
[composition service](../transflow/BRANCHES.md) requires current complete validation
and a writable runtime owner before invoking it.

`input_resolution` chooses and leases exact published heads atomically, with
per-alias fallback evidence and publication provenance. See [INPUTS.md](INPUTS.md).

T044 adds audited branch lifecycle and immutable source-indexed authored policy
snapshots; see the [branch guide](../transflow/BRANCHES.md). T045 adds exact historical
overrides and schema-5 retained replay manifests with atomic boundary leases;
see [pins and replay](REPLAY.md).

[T051 cache reuse](CACHE.md) adds schema 6 and immutable cached-job evidence.
Branch-scoped candidates retain original check results; audited head adoption
emits head changes without new materializations. There are now 46 logical tables
plus the migration ledger.


[T052 freshness](../transflow/FRESHNESS.md) adds schema 7, immutable computation
comparison evidence, and a single-transaction head/attempt/check snapshot. There
are now 47 logical tables plus the migration ledger. Existing publications remain
unchanged; absent comparison metadata is explicitly unknown.


[T063 check evidence](../tf-exec/CHECKS.md) adds schema 8: complete immutable
check envelopes, private bounded samples, certificate keys, explicit reuse records
and failed-candidate retention roots. Original evaluation times never change on
reuse. Diagnostic reads require explicit intent and return warnings; safe evidence
reads do not load row values. Consumer failures cannot rewrite provider results.


[T070–T072 foreign data](../transflow/EXTERNAL.md) retains immutable foreign metadata
without installing local input artifacts. Schema 10 retargets qualified foreign
job-input FKs from replicas to foreign_versions while preserving history and legacy
files. Local version/parent-job constraints remain intact. The application obtains
and renews provider leases; retained metadata alone cannot grant a local data read.


## Current schedule snapshots (T090)

`Store::save_schedule` uses `BEGIN IMMEDIATE` to compare the opaque edit ETag,
overwrite the only current snapshot, rotate its trigger epoch/event cursor,
clear unconsumed tokens, audit the edit and optionally commit its API receipt.
Replacement preserves paused state. `Reader` exposes bounded current records.
`freeze_schedule_occurrence` retains accepted build settings/policies independently;
it does not evaluate triggers or enqueue execution. Pending accepted execution
and current fixed-source definitions protect source captures from collection and
block deletion of referenced data branches. Completed occurrences are historical
metadata, rather than permanent source retention roots.

## Durable schedule event evidence (T091)

`Store::scan_schedule_events` scans up to 100 committed events for an expected
current trigger epoch. Matching tokens, the event cursor and scan audit commit
in one transaction. Evidence format 1 retains the original timestamp, sequence,
causation/correlation and exact origin-qualified dataset version. Token expiry
uses event time. Paused schedules advance their cursor and count matching events
as ignored; definitions awaiting review advance without evaluating legacy rules.
Per-event payloads are capped at 256 KiB, each page at 1 MiB, and successful-build
job inspection at 10,000. Invalid evidence or a storage failure rolls back the
page; continuation is explicit.

Publication differs from cached head adoption. Head-reset matching is opt-in.
Publication/cache events now freeze the branch name in their visibility transaction;
legacy events without that field use retained branch metadata. Token IDs are
stable across cursor replay within one schedule/epoch/leaf/event combination.
Signal-only tokens keep the observation, but `TokenPayload::input_pin` returns no
binding for them.

`receive_foreign_schedule_event` is a trusted repository adapter requiring an
already registered provider and retained immutable version metadata. It deduplicates
publications by origin dataset/version and head changes by origin dataset/branch/
generation. Conflicting redelivery fails closed. It opens no provider, installs no
replica and copies no bytes. Metadata retention does not guarantee provider byte
availability; later execution must obtain provider leases.

`complete_schedule_success` records occurrence success and its event atomically,
only for its own successful build with all required target jobs successful or cached.
For `require_materialization`, at least one required target must have executed
successfully; all-cached success does not qualify. Retries reuse the same event.

Local versions from unprocessed committed head events are conservatively protected
while live automatic schedules have not scanned them. Unexpired unconsumed tokens
and pending occurrence `payload_json.tokens` retain exact local versions and their
transitive provenance. Accepted/queued/held/running occurrence pins survive token
expiry and definition edits; cancellation/completion releases these pending roots.
Existing retention traversal limits still fail closed. No AND/OR consumption,
cron evaluation, automatic provider polling, enqueue or dispatch is implemented here.
