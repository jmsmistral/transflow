# T012 wire contract baseline

[`contracts-v1.schema.json`](contracts-v1.schema.json) is the single authored
contract source, exposed by `tf-protocol` through `CONTRACT_SCHEMA`. Consumers
select a named `$defs` entry; the document root is a definition collection, not a
validator for arbitrary payloads. These definitions implement architecture 7.1
and the shape of the future worker boundary. They do not start a worker or publish
artifacts. T014 now implements protocol 1.0 framing; installed SDK/worker diagnostics
report 1.0 with no execution operations enabled.
[Production domain types](../crates/tf-domain/README.md) are implemented by T013.
[Transport/code generation](../crates/tf-protocol/README.md) is implemented by T014;
[Canonical encoding/hashing](canonical-v1.md) is implemented by T015.

## Independent versions and compatibility

[`versions.json`](versions.json) records independent baselines:

| Format | Baseline | Status |
|---|---|---|
| Worker control protocol | major 1, minor 0 | Framing and session guards implemented by T014 |
| Logical schema, artifact manifest, catalogue snapshot | 1 each | Shapes defined here |
| Expectation AST, workspace config, authoring registry, HTTP API | 1 each | Reserved; their full schemas remain with later tasks |
| SQLite schema | 0 | No product migration exists; not permission to adopt an arbitrary database |

Product version, specification version and these format versions are independent.
Persisted documents accept only their exact known format version; unsupported
versions fail before state is used or modified. There is no automatic downgrade,
recreation or migration. Changing one format does not bump every other format.

Worker peers must agree on the major version. A higher minor version is compatible
only when the actual message and required capabilities are understood. Core
objects are closed: unknown members, missing required members, unknown tags and
unknown operations fail. Adding a required field, removing/changing a field or
changing its meaning requires a new major version (or new persisted format).
An additive minor feature uses a named extension/capability, with its own explicit
shape. Senders emit it only after the receiver advertises support; an old receiver
must reject an unsolicited unknown extension, even when it is not marked required.
Absence of an optional extension preserves the baseline behavior. Ordinary field
metadata is an open string-to-string map, without executable meaning.

The fixtures use a deliberately small receiver profile that understands only
`diagnostic.note.v1`, carrying a string scalar. T014 implements the same optional diagnostic extension, gated by both peers
advertising support. Worker execution operations remain unavailable. Required capability names and extension names must be unique, and
there can be at most 64 of each. The hello capability advertisement has the same
64-item bound. T014 enforces hello, identity, ordering and capability intersection; T055 owns
nonce authentication and the process launcher.

## Values and identities

All integer values, file byte/row counts and frame sequences are decimal strings.
Signed/unsigned 8/16/32/64-bit ranges are checked; leading zeros, a leading plus,
negative zero and exponent notation are rejected. Small version/precision/scale
numbers remain JSON numbers and cannot be booleans. UUIDs use lowercase hexadecimal
with 8-4-4-4-12 grouping; digests use 64 lowercase hexadecimal characters. UUID
version/variant allocation and generation remain domain responsibilities. Dataset
identity is `(workspace_id, dataset_id)`, independent of its display path.

| Value | Wire representation and assertions |
|---|---|
| Null | `{"type":"null"}` with no value member |
| Boolean/string | Tagged JSON boolean / Unicode string; strings remain text |
| Binary | Tagged canonical padded RFC 4648 base64, including empty data |
| Integer | Width/signedness tag and range-checked decimal string |
| IEEE float | Width tag and decimal string, or exactly `NaN`, `Infinity`, `-Infinity`; signed zero retained |
| Decimal128 | Decimal string plus precision 1–38 and scale −38–38; preserve exact trailing fractional digits |
| Date | Gregorian `YYYY-MM-DD`, years 0001–9999, with actual day/leap-year validation |
| Timestamp | ISO calendar text plus explicit `s`, `ms`, `us` or `ns` unit and nullable timezone |
| List/struct | Recursive tagged values; struct fields are ordered and uniquely named |

Float decimal syntax is JSON-number syntax inside a string. Finite text must
convert to a finite value of the declared width; a nonzero mantissa cannot underflow
to zero. NaN is distinct from null; individual NaN payload bits are not represented.
Writers must emit decimal text that round-trips their IEEE value. Canonical JSON v1
preserves the supplied validated string exactly; it does not collapse equivalent
IEEE decimal spellings. This conservative content identity retains signed zero and
avoids cross-runtime float reformatting. The fixture readers also retain text.

For positive decimal scale, exactly that many fractional digits are required;
precision counts digits of the unscaled integer, ignoring leading zeros (zero
counts as one digit). Scale may exceed precision. At scale zero, no decimal point
is allowed; at negative scale, the integer value must be a multiple of the
corresponding power of ten. No exponent or implicit float conversion is allowed.

Naive timestamps have no suffix and no timezone assumption. Aware timestamps store
the instant as UTC text ending in `Z`, alongside the original named timezone.
Fractions have exactly 0/3/6/9 digits according to the unit; leap seconds and numeric
UTC offsets in the text are rejected. The timezone carrier accepts `UTC` or slash-
separated IANA-style names; checking zone existence and engine representability
requires later adapters. These calendar strings do not promise every engine can
represent every year at every unit. Unsupported conversions must fail explicitly.

Logical fields have a name, type, nullability and optional metadata. Names are
1–256 Unicode scalar values without ASCII controls or DEL. Lists carry an element
field, structs carry ordered fields; field names must be unique within each schema
or struct. Nested values are a representation contract, not a dataframe transport.

Dataset path segments use `[a-z][a-z0-9_]*` and exclude Python hard keywords.
Catalogue snapshots reject duplicate paths/identity pairs. Local entries belong
to the snapshot workspace; foreign entries use kind `external` and the reserved
`external/` path prefix. Artifact paths are nonempty relative paths of at most
1024 Unicode scalar values, with no empty/dot/parent segments, backslash, colon,
ASCII controls or DEL. Filesystem containment, symlinks and case sensitivity need
actual filesystem checks later; this lexical check alone is not a sandbox.

## Artifacts and control messages

An artifact manifest carries ordered file entries, a logical schema and its
fingerprint, and writer engine/version/compression/row-group settings. Every file
has a path, digest and string byte/row counts. At least one file is required even
for an empty table; duplicate file paths fail. Engine identifiers include future
adapters without claiming those adapters exist. T015 checks the logical-schema
fingerprint before hashing a manifest. Physical file contents/lengths and normalized
writer settings still require later storage verification. The artifact digest covers
the canonical manifest (including ordered file digests), and is not embedded
recursively in the manifest itself.

Each control envelope has protocol major/minor, request and attempt UUIDs, a
sequence, required capabilities, extensions and one discriminated message. Defined
messages are hello, phase, heartbeat, metric, artifact_ready, check_results,
completed and error. Hello names one of the six architecture operations. Metrics
carry scalars; artifacts and check results travel by relative paths plus digests,
not inline tables or pickle. File sizes, sequence monotonicity, lifecycle ordering,
request payload schemas and error propagation remain production transport work.

Architecture 2.4 requires a private Unix socket, four-byte big-endian length prefix,
UTF-8 JSON capped at 1 MiB, and separate stdout/stderr logging. T012 tests decoded
JSON shapes; T014 now enforces byte/depth limits, strict JSON, framing and session
guards in Rust and Python. Full log backpressure, authentication and child lifecycle
remain T055. Arrays in persisted schemas have no arbitrary small item cap; each
reader owns its byte/depth limits.

## Conformance checks

The JSON Schema uses Draft 2020-12 structural keywords plus asserted custom
`transflow-*` formats and `x-transflow-invariant` rules. A generic schema validator
that ignores these annotations is **insufficient**. The named invariants enforce
decimal precision/scale, timestamp unit/timezone, unique field/file names,
catalogue ownership and the receiver capability profile described above.

Three independent, test-only assertion readers consume this one schema and the
same fixtures: Rust `crates/tf-protocol/tests/contracts.rs`, Python
`python/tests/contracts/assertions.py`, and TypeScript `web/src/contracts.test.ts`.
They support only the documented schema subset, reject unknown rules and use a
64-call recursion guard. That guard bounds the fixture interpreter, not a promised
production nesting limit. These are not general JSON Schema engines or installed
runtime APIs. No test helper is shipped inside the Python wheel.

Run `cargo test -p tf-protocol --locked --offline`, the Python package check runner
and the web test runner. The shared corpus has 157 value/identity/schema/message
cases and 17 independent version cases, plus each reader's unknown-rule test.
Positive fixtures survive JSON serialization unchanged; negative fixtures exercise
range/precision/shape/version/capability failures. Native engine conversion and
socket conformance remain later A10/A37 work. T014 additionally runs the shared corpus through runtime validators and registers
its generated schema, TypeScript and Python outputs with deterministic drift checks.

T015 fixes [canonical JSON and hash domains v1](canonical-v1.md), with shared fixed
[golden vectors](fixtures/canonical-v1.json). Generic canonical metadata is bounded
to 16 MiB, distinct from the 1 MiB control-frame cap. The protocol stays at 1.0.
