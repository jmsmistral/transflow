# Wire contracts and transport (T014)

`tf-protocol` provides runtime validation of the authored T012 schema, immutable
validated control frames, bounded UTF-8 JSON framing and per-request receive guards.
It uses the already-qualified Serde libraries and pure `tf-domain` constructors.
It contains no worker operation dispatch, source imports, process launcher or HTTP
server. Python counterparts ship inside the single `transflow` wheel.

## Transport contract

`read_frame` and `write_frame` take an explicit blocking channel. Payloads contain
1–1,048,576 UTF-8 bytes after a four-byte unsigned big-endian length prefix. Readers
check length before allocating a body. Writers bound encoded output and emit no
bytes when encoding exceeds the cap. Clean EOF before a prefix differs from
truncated prefixes/bodies. Partial I/O is handled; read deadlines, cancellation and
connection shutdown remain the caller/supervisor's responsibility.

Decoding rejects duplicate keys at every object level, non-finite JSON numbers,
invalid Unicode, trailing JSON and nesting beyond 64 value levels. Schema
validation adds the T012 custom format and invariant checks, including domain
numeric/calendar validation. JSON wire formatting is not canonical hashing (T015).
Rust `ControlFrame::from_json` validates structured input; encoding still applies
the byte cap. Python frames retain immutable validated bytes and return detached
mappings so caller mutation cannot change an accepted frame.

The codec receives protocol major 1. Minor 0 is current; a higher advertised minor
is acceptable only with understood message shapes and capabilities. The installed
SDK/worker compatibility diagnostic now reports 1.0 and `wire_protocol_implemented`
as true. `supported_operations` advertises `discover` and `execute`; T058 enables
the `polars.execute.v1` capability with `PolarsExecutionRequestV1`. Required engine
packages are checked at execution, never installed implicitly. Check/query workers
remain later work.

`Session` binds request UUID, attempt UUID and an expected operation before reading
hello. It requires hello first and only once, checks operation and identity,
intersects advertised capabilities, and enforces strictly increasing sequences.
The first sequence may be any u64; gaps are allowed, replay/decrease is not. A
completed/error message is terminal. Rejected messages do not advance state.
The initial negotiated minor is zero. Minor compatibility never substitutes for
explicit feature support.

The optional `diagnostic.note.v1` extension carries a string scalar and is accepted
only if both peers advertise it. Unknown advertised optional capability names can
be excluded from the intersection; unknown required capabilities or extension
payloads fail. Hello advertisements must contain unique capability names. Message
objects and operation tags remain closed unions. Metrics contain scalars; artifact
and check results use paths/digests rather than inline tables.

The schema supports six operation identifiers and eight message types. Frames carry
separate typed request/attempt IDs and lossless sequence counters. Session acceptance
is a prerequisite for future dispatch, not authorization to execute user code.
Per-attempt nonce authentication, private socket creation, bounded log draining,
heartbeat diagnostics and child cleanup are implemented by the [T055 launcher](../tf-exec/SUPERVISION.md). T027 implements real
discovery. This library does not replace those tasks or enable CLI execution.

## Generated clients and schema exports

Run `python tools/contracts/generate.py` from the implementation checkout. The
stdlib-only deterministic generator uses `schemas/contracts-v1.schema.json` and
`schemas/versions.json`, exposed by Rust constants, as its single authored source.
It emits:

- `schemas/generated/worker-control-v1.schema.json`: standalone frame entry point
  with all referenced definitions and asserted custom formats.
- `web/src/generated/contracts.ts`: readonly discriminated TypeScript types.
- `python/worker/src/transflow_worker/_wire_schema.py`: embedded schema/version data.
- `python/worker/src/transflow_worker/_wire_validators.py`: named runtime validators.

TypeScript declarations provide static shape checking, not runtime validation of
network data. Generic JSON Schema tools must assert the custom Transflow formats;
ignoring annotations is insufficient. Python uses the shared assertion engine;
Rust uses domain constructors for numeric/calendar/path assertions. Neither engine
claims to implement every JSON Schema vocabulary.

`security/generated-contracts.json` records the reviewed generator/input hashes
and all outputs. The safety gate regenerates in a disposable directory and compares
exact bytes; schema or generator changes require reviewing outputs and refreshing
those input digests. Generation needs no Node, Rust build or network access.
Installed Python validation needs neither checkout nor the specification repository.

## Verification and limits

The runtime Rust and generated Python validators each run all 157 shared T012
schema fixtures. Rust transport tests cover every truncation position, fragmented
I/O, byte boundaries, invalid JSON/version/tag input, negotiated capabilities,
identity mismatch, replay and terminal ordering. A real Python subprocess exchanges
frames with Rust over a mode-0600 Unix socket in a mode-0700 temporary directory,
while JSON-looking stdout and stderr remain separate logs. The test uses `python`
from the prepared PATH, or `TRANSFLOW_TEST_PYTHON` when explicitly supplied; macOS
accepted sockets are explicitly set to blocking mode. It does not import user code.

Python tests add strict malformed-frame cases, short writes, immutable-frame
behavior, installed-wheel validation and session failures on both supported minor
versions. TypeScript checks generated tagged types, including compile-time rejection
of numeric wide-integer carriers and unknown message tags. No browser is required.

Run the existing Rust/Python/web and safety runners. Local environments that block
Unix sockets must grant that test local IPC access; skipping it is not a pass.
Live engine execution remains later work. T055 adds native authentication,
cancellation and bounded-log failure tests, alongside installed discovery tests.

T015 adds the `canonical` module for bounded JSON encoding, purpose-separated SHA-256,
streamed raw file hashes and validated manifest/catalogue projections. See the
[byte contract and limits](../../schemas/canonical-v1.md). Full source/compute field
selection, object verification and publication remain later services.

T016 adds `diagnostic::CliEnvelope` and the standalone generated CLI result schema.
The current shared corpus has 167 cases; five schema/type/validator outputs are
registered. Worker protocol 1.0 is unchanged. CLI envelope version 1 has independent
output/status/context semantics described in the [CLI guide](../transflow/README.md).

T059 adds [expectation AST-v1](../../python/EXPECTATIONS.md) decoding, typed domain
syntax, schema-aware semantic preflight and canonical re-encoding. Discovery and
Polars execution now require `expectation.ast.v1`; peers lacking it fail closed.
The catalogue normalizes the readable legacy two-operation seed before hashing.
Shared semantic vectors qualify canonical AST bytes and policy-resolved hashes;
this module does not evaluate data or compile SQL.
