# Runtime ownership (T019)

`ownership::RuntimeOwner` holds an OS file lock for its lifetime. It is neither a
PID-file lock nor a running coordinator service. `serve` and the local API remain
later work. T055 adds [worker supervision](SUPERVISION.md), now used by discovery;
the [public CLI](../transflow/README.md) exposes preparation and inspection commands.

The caller supplies an explicit workspace directory and durable workspace UUID.
`.transflow/runtime` must already exist, belong to the current user and have private
0700 permissions. Metadata/lock files are private regular files, never symlinks or
hardlinks. Safe descriptor-relative filesystem calls avoid following substituted
file paths. The guard validates root/runtime device and inode and the locked file's
identity before publishing metadata or handing out its owned repository.

A second process cannot acquire the same lock. The OS releases it on process exit or
kill. Normal guard/probe release explicitly unlocks before closing the descriptor,
so a concurrent fork cannot briefly prolong ownership through an inherited descriptor.
Lock files are never removed during shutdown, avoiding a second lock inode.
Metadata may remain stale; discovery treats an unlocked runtime as inactive regardless
of its PID file. Only a new lock holder can atomically replace registration, with file
and directory synchronization. All ownership I/O is synchronous and belongs outside
an async request executor; DB access itself is async.

## Discovery and modes

Registration records an independent runtime format/protocol version, canonical root
and runtime identities, durable workspace UUID, process PID/start identity, random
session UUID, 256-bit nonce, mode and a bound loopback TCP endpoint. Randomness comes
from the OS; Debug output excludes the nonce. JSON input is bounded to 16 KiB and
rejects unknown fields, unsupported versions and malformed identities/addresses.

Linux process identity combines boot ID and `/proc` start ticks. macOS uses the OS
`ps` start record with a fixed locale; this has one-second precision. Neither is a
standalone authorization credential. A returned registration is a discovery candidate:
the future local API must prove the exact random session nonce before attaching or
sending mutations. No network connection or authenticated API handshake is implemented
here. Process/lock state can change after discovery; callers must handle that race.

Discovery inspects only the explicitly supplied runtime, never a global UUID index.
Copied metadata in another checkout fails location checks even with the same durable
workspace ID. Explicit associations for user Git worktrees remain future integration.
The owner directory is trusted against hostile modifications by the same OS user;
replacing/unlinking a held lock is unsupported and detected before further operations.

| Mode | Active schedules | No-wait acceptance | Purpose |
|---|---|---|---|
| Persistent | Allowed | Allowed | Explicit foreground serve |
| Temporary | Disabled | Rejected | One mutating command |
| MetadataOnly | Disabled | Rejected | Provider metadata/export leases |

Provider operations must attach to the existing owner or acquire this same lock for a
short-lived metadata-only coordinator. They must not open a second unmanaged SQLite
writer, discover/import Python, run producers or activate schedules. These are mode
contracts for future service dispatch; there is no producer/scheduler service yet.

`open_store` borrows the guard mutably, allowing one owned store at a time and keeping
ownership alive across repository use. Always await `OwnedStore::close` before
coordinator shutdown; dropping an async operation can leave an uncertain commit
outcome, so recovery must consult durable state and later reservation fences. This
foundation does not supervise worker cancellation or implement recovery publication.

## Verification

Nine tests include one subprocess fixture entry point and a deterministic inherited-descriptor regression. Parent tests launch a real
owner, wait on a readiness marker, prove contention, kill/reap it and reacquire with
a different session. They cover stale metadata, copied clones, incompatible schemas,
process identity, unsafe files/permissions, replaced locks, private atomic registration,
loopback-only endpoints, mode policy and the owned database lifetime. A21/A63 service,
CLI disconnect and cancellation scenarios remain later tasks.

A macOS CI recurrence during T020 was reproduced as a failed immediate reacquisition
after dropping an owner. A deterministic test keeps a duplicate locked descriptor in
a live child: closing alone fails, explicit unlock permits reacquisition before the
child exits. The guard and temporary discovery probes now explicitly unlock. This
fix preserves contention while the guard is live and does not extend test timeouts.
