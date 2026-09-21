# Worker module

`transflow_worker` ships in the same `transflow` wheel as the SDK, sharing its
version. `python -I -m transflow_worker` is the isolated module entry point;
`transflow-worker` is the diagnostic console entry point.

Help, version and `compatibility` are available. The latter checks installed
package identity and protocol metadata. Captured `discover` imports use a mutually
authenticated private socket with bounded framed control, independent of stdout
and stderr. The Rust [supervisor](../../crates/tf-exec/SUPERVISION.md) owns bounded
logs and child-group cleanup. The private `execute` operation now runs the
[Polars adapter](../POLARS.md) with pinned scans and a single streaming sink.
Artifact-ready evidence still requires Rust byte verification, output checks and
guarded publication; public dataset builds and other engine adapters remain later work. Diagnostic stdout is separate from worker control. See [setup and checks](../README.md).
