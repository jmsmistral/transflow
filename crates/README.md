# Rust source boundary

The T004 workspace contains the ten specified crates. Crate responsibilities and
permitted dependency direction are
canonical in [architecture section 2.2](../../transflow-spec/TECHNICAL_ARCHITECTURE.md#22-rust-modules-and-dependency-direction).

Only `transflow` has executable behaviour: help, version, argument diagnostics and
typed output errors. The nine `tf-*` libraries reserve their boundaries without
implementing domain, storage, execution or service behaviour.

[check_rust.py](../tools/check_rust.py) enforces allowed dependency directions,
cycles, exact qualified dependency versions/sources and workspace lint inheritance.
The domain crate starts with no dependencies. New external dependencies require
an explicit boundary and qualification review; permitted edges do not require
unused dependencies to be added. All crates forbid unsafe code and inherit Clippy
rules against panic/unwrap/expect/todo scaffolding.

Run `bash tools/check-rust.sh` after `cargo fetch --locked`; see the
[verification contract](../docs/development/verification.md). The root lockfile is
separate from T003's standalone probe and currently contains a subset of its
qualified package versions and checksums. T003 qualification remains pending in
CI; the [compatibility report](../docs/development/compatibility.md) records the
available native evidence.
