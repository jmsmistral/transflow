# Rust source boundary

The T004 workspace contains the ten specified crates. Crate responsibilities and
permitted dependency direction are
canonical in [architecture section 2.2](../../transflow-spec/TECHNICAL_ARCHITECTURE.md#22-rust-modules-and-dependency-direction).

The `transflow` executable provides help, version, argument diagnostics and typed
output errors. T013 implements the pure [tf-domain library](tf-domain/README.md)
for IDs, names, schemas, values and location-aware errors. Storage, execution and
service behaviour remain unimplemented. T012 exposes the
authored [wire schema](../schemas/README.md) through `tf-protocol` constants and
checks shared fixtures using the already-qualified serde_json as a dev dependency.

[check_rust.py](../tools/check_rust.py) enforces allowed dependency directions,
cycles, exact qualified dependency versions/sources and workspace lint inheritance.
The domain crate still has no third-party dependencies; tf-protocol imports it only
in tests to project shared fixtures through its constructors. T007 adds SQLx/bundled SQLite
and Tokio only as `tf-store` dev dependencies for the [integration fixtures](../tests/README.md).
Tokio is permitted there only for tests; the checker rejects normal/build promotion. New external dependencies require
an explicit boundary and qualification review; permitted edges do not require
unused dependencies to be added. All crates forbid unsafe code and inherit Clippy
rules against panic/unwrap/expect/todo scaffolding.

Run `bash tools/check-rust.sh` after `cargo fetch --locked`; see the
[verification contract](../docs/development/verification.md). The root lockfile is
separate from T003's standalone probe and currently contains a subset of its
qualified package versions and checksums. T003 qualification and the T004 Rust CI matrix now pass; the [compatibility report](../docs/development/compatibility.md) records the
available native evidence.
