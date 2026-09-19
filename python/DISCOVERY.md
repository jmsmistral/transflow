# Isolated discovery (T027)

The installed worker now supports the internal `discover` operation. There is no
new public discovery command or trust ceremony. A coordinator supplies a validated
`DiscoveryRequestV1` in a private mode-0600 file and a socket in a canonical,
mode-0700 directory. It launches a fresh managed interpreter with `-I -B -m
transflow_worker discover --request ... --control-socket ...`. Environment drift
checking through T025 is a caller precondition; discovery never resolves or
installs packages. Public build/validate orchestration remains later work.

Before imports the coordinator sends the request's random 32-byte challenge over
the dedicated socket. A mismatch aborts without importing source. Control frames
then use the T014 length-prefixed protocol and `discovery.v1` capability, with
hello, phase, heartbeat, discovery_ready and completed/error messages. The ready
message names a size-bounded result file and its byte digest. Stdout/stderr are
separate logs; T055 still owns the general launcher, bounded log draining,
cancellation and wall deadlines. There is no worker-side default memory cap.

Discovery validates catalogue fingerprints, source paths/hashes and the T022 module
index, and compiles every importable Python file before importing any of them.
Captured code is loaded from those verified, compiled bytes, so later changes to a
captured `.py` file cannot change a subsequent ordinary import in this worker.
Packages, namespace packages and relative/helper imports use normal module caching.
A finder rejects uncaptured children beneath indexed source namespaces. This is
not protection against deliberately hostile Python or arbitrary filesystem/network
access: **module-level code executes**, while producer functions are never invoked.

Each originating module may contain zero or one producer. Imported/re-exported
functions and repeated names pointing to one function are not duplicate producers.
Discovery retains unresolved strings for T028's same-pass resolution, source
locations, independent branch/fallback policy, checks, schemas, typed parameters,
source refresh and secret reference names. The worker binds a private immutable
adapter for CatalogSnapshotV1 before imports, retaining foreign owner IDs.
Production public catalogue/testing APIs and alias-expanded projections remain
T030. No database, catalogue identity or dataset version is written here.

Import failures (including a module's SystemExit) yield DiscoveryDiagnosticV1 with
a safe explanation, exception class and source location. Arbitrary exception text,
source snippets and request tokens are not copied into diagnostics. Syntax/index
failures precede imports. Results are created once in a private attempt directory;
partial/failed results are never announced as ready. The coordinator reads only
schema-valid, digest-verified results after successful completion and owns cleanup.

Limits are explicit: 1 MiB request/control payloads, 16 MiB per source file and
metadata result, 256 MiB aggregate source bytes, and the existing module/root/count
limits. These declaration-service bounds are independent of future transform
memory policy. Captures containing oversized resources fail explicitly. A fresh
process per request prevents leaked module state between workspaces.

Tests build and install the actual wheel, launch fresh interpreters over real Unix
sockets, and verify source guards, helper imports, re-exports, import exceptions,
C ownership, separate print output, policy serialization and unchanged producer
counters. Shared fixtures exercise Rust, Python and TypeScript schema readers.
