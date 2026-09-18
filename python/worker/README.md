# Worker module

`transflow_worker` ships in the same `transflow` wheel as the SDK, sharing its
version. `python -I -m transflow_worker` is the isolated module entry point;
`transflow-worker` is the diagnostic console entry point.

Help, version and `compatibility` are available. The latter checks installed
package identity and bootstrap protocol metadata. Execution, discovery, engine
adapters and framed socket control remain unimplemented. Diagnostic stdout is
not the future worker control transport. See [setup and checks](../README.md).
