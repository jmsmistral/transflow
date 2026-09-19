# Deterministic integration-test support

T007 supplies reusable boundary fixtures under `support/`, exercised by the
`tf-store` integration target `infrastructure`. Everything here is compiled only
for tests; none of these providers is a production implementation or exported SDK.
Run after the explicit locked Cargo dependency setup:

```bash
cargo test -p tf-store --test infrastructure --locked --offline
bash tools/check-rust.sh
```

The existing Rust CI matrix runs this target through `cargo test --workspace` on
macOS arm64 and Linux x86_64/arm64. Local evidence covers macOS arm64; the [three-platform CI run](https://github.com/jmsmistral/transflow/actions/runs/35410393011)
also passes at `a5191b9`. No browser or new system dependency is installed for these tests.

## Fixture contracts

- `VirtualClock` exposes explicit monotonic time and checked advancement. Clones
  observe the same clock; advancing time does not release a publication barrier.
  It has no connection to production phase timers or the system clock.
- `SequentialIds` fails on exhaustion; `SeededRandom` uses a fixed SplitMix64
  sequence. Set each test's seed/starting ID explicitly. These are synthetic
  providers, never production UUIDs, secrets or probabilistic uniqueness claims.
- `barrier(label)` returns a one-shot checkpoint and controller. Wait for arrival,
  inspect real state, then release with Continue, Cancel, NoSpace or CoordinatorLost.
  Channel closure fails visibly. Ten-second watchdogs detect a broken test;
  ordering comes from messages, not real sleeps. Future production hooks must
  use the relevant boundary label rather than silently skipping a required hook.
- `ScratchDirectory` atomically creates a directory it owns and removes it during
  error/unwind cleanup. `close()` reports cleanup errors; Drop reports them without
  replacing an earlier failure. It never adopts or recursively scans a user path.
- `RealFiles` uses create-new, writes and actual fsync. `FaultFiles` interposes only
  at labelled I/O boundaries. ENOSPC is an injected OS error (28 on the supported
  macOS/Linux platforms); tests do not fill the user's disk. Paths supplied to
  these helpers must be inside the test's owned fixture directory.
- `database` opens actual SQLx 0.9.0 / bundled SQLite 3.51.3 with WAL, FULL sync
  and foreign keys. The [tiny SQL schema](../fixtures/harness.sql) is synthetic;
  it is not a production catalogue or migration. Tests inspect transactions and
  reopen the database after a process is killed.
- `TestChild` launches the current Rust test executable with an argument array and
  an explicit fixture root. It holds a transaction until the parent sends a byte
  or closes stdin. Output is capped at 4096 bytes by the readiness reader; stderr
  is captured inside the fixture. Explicit finish and Drop reap the owned child.
  Readiness and output completion have watchdogs. This controlled child retains
  stdout until exit and never spawns descendants. Do not reuse this helper as a
  general worker supervisor: process groups, descendant cleanup, PID fencing and
  arbitrary untrusted logs need the later execution implementation.

The stdout marker and single-byte stdin commands are harness-only synchronization,
not the future framed worker control protocol. A37 framing/log isolation remains
unimplemented. A20 full recovery also remains open: the tests use an intentionally
small head update to qualify the infrastructure, not a mock proof of Transflow's
publication transaction, durable outbox, Parquet validation or orphan recovery.

## Dependency boundary

SQLx, libsqlite3-sys and Tokio are `tf-store` dev dependencies, using T003's exact
pins and lock identities. The boundary checker permits Tokio for `tf-store` only
as a dev dependency; a regression rejects runtime/build promotion. No extra crate
or production dependency edge is introduced. Future crate test targets may reuse
support modules, adding only the fixtures and qualified dev dependencies they need.
