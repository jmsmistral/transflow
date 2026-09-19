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

Two additive migrations install all 40 logical tables: source/catalogue projections,
versions/heads, builds/jobs/attempts, checks, scheduling, events/outbox, pins/audit,
registry mutation journals and foreign replicas/leases. These tables are foundations,
not delivered scheduler, publication, retention or replica-copy services.

The database has an application ID, `user_version` and monotonic checksum ledger.
Migration checksums are SHA-256 of exact embedded SQL bytes. Open rejects unrelated
or newer databases, missing/reordered ledger entries and changed checksums. All
pending DDL and version markers commit in one IMMEDIATE transaction; failure rolls
back the whole upgrade. Existing migration bytes must never be edited after delivery.
No destructive migration is currently provided. Future destructive upgrades require
an explicit backup policy and migration before they can ship; there is no downgrade.

STRICT tables, foreign keys, compound head/version identity, unique input aliases,
attempt numbers and occurrence evidence constrain writes. Immutable snapshots,
versions and declarations reject updates; terminal attempts cannot be rewritten.
Events, head changes and audit evidence are append-only. Head generations must
increase. A version's producing attempt must belong to that dataset. Service-level
rules and atomic head/event/publication operations remain later work.

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
before retrying. This foundation does not implement coordinator cancellation semantics.

Nine real-file tests cover fresh/reopen/upgrade, compatibility/checksum refusal,
foreign keys and identity constraints, injected migration/audit failure, bounded
contention, concurrent WAL snapshots, immutable evidence and symlink refusal.
A20/A39 crash recovery, backup/restore and full publication remain later tasks.

[Logical normalization](NORMALIZATION.md) projects Arrow/Parquet schemas, validates
actual values, emits bounded typed cells and exposes per-adapter schema guards.
The import service retains this evidence without publishing artifacts or heads.
