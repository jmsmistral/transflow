# Domain identities and values (T013)

This crate implements pure, validated domain types without third-party dependencies,
filesystem access, catalogue mutation or engine imports. Constructors return
`DomainError` with a typed `ErrorKind` and a relative JSON Pointer location. Error
messages do not echo arbitrary input data. JSON envelopes, framing and generated
contracts are implemented by [tf-protocol](../tf-protocol/README.md) in T014.

| Module | Delivered types |
|---|---|
| `identity` | RequestId (T014), WorkspaceId, DatasetId, VersionId, AttemptId, BranchId, SourceSnapshotId, DatasetKey, DatasetScope |
| `path` | DatasetPath with syntax and ownership-scope validation |
| `branch` | BranchName, BranchSelector and independent FallbackPermission |
| `schema` | FieldName, Field, unique ordered Fields, LogicalType and LogicalSchema |
| `value` | Checked integers, IEEE carriers, exact decimal coefficients, dates, timestamps, scalars and nested values |

```rust
use tf_domain::{DatasetKey, DatasetPath, DatasetScope};
use tf_domain::value::{IntegerType, IntegerValue};

let key = DatasetKey::from_text(
    "00000000-0000-4000-8000-000000000001",
    "00000000-0000-4000-8000-000000000002",
)?;
let path = DatasetPath::parse_for_scope("raw/orders", DatasetScope::Local)?;
let count = IntegerValue::parse(IntegerType::U64, "18446744073709551615")?;
```

A key contains only UUID identity, not a path or branch. Renaming a display path
cannot change the key, and identical dataset UUID bytes in different workspaces
produce different keys. ID types cannot be assigned to one another accidentally;
a compile-fail documentation test enforces that distinction. Parsing accepts the
T012 lowercase UUID carrier without imposing a version/variant allocation policy.
`from_bytes` preserves existing UUIDs; neither it nor parsing generates an ID.
Random allocation and durable registration belong to mutating catalogue operations.

`DatasetPath::parse_for_scope` validates both syntax and use of the reserved
`external/` namespace. Plain `FromStr` validates only path syntax so the same type
can represent local and registered foreign names. Callers with a known owner must
check scope. Errors identify the invalid segment, for example `/segments/1` for
`raw/class`. Parsing or classifying a name never registers it or grants permission
to execute a foreign producer. Namespace-only resolution remains catalogue work.

Branch names preserve exact Unicode, case and slash characters. They are nonempty
metadata strings without Unicode control characters; they are not Git ref or
filesystem path components. No trimming, case folding, slash-to-directory mapping
or ambient Git lookup occurs. `Omitted` remains distinct from `Current` for future
foreign-provider defaults. `Named` does not imply prohibited fallback. This module
represents declarations; actual policy resolution remains later planning work.

Numeric constructors follow the [T012 carriers](../../schemas/README.md). Integers
use exact i128 storage constrained to their declared 8/16/32/64-bit width. Decimals
retain their original fixed-point text, precision/scale and an exact i128 unscaled
coefficient. Float carriers retain text and IEEE bits (binary32 widens exactly for
the native accessor); NaN, null and negative zero stay distinguishable. Equality
of these carriers compares representations, not SQL or numerical equivalence.
Canonical numeric spelling and hashes remain T015.

Dates use Gregorian validation; timestamps retain 0/3/6/9 fractional digits and
explicit naive/named-zone semantics. Aware text is UTC with a `Z` suffix while the
original zone remains metadata. This crate checks zone-name syntax, not membership
in an installed timezone database. It does not convert to an epoch integer or
claim every engine supports every representable calendar value. Adapters must
still reject unsupported/lossy conversions.

Logical fields preserve nullability, ordered nesting and metadata, including
absent versus explicitly empty metadata. Field names count Unicode scalar values,
not bytes or UTF-16 code units. Private field collections prevent duplicate names
being introduced after construction. Raw binary values contain bytes; base64 and
JSON object validation remain codec responsibilities. Domain constructors do not
validate a whole value against a declared schema or apply engine/check semantics.
Transport must bound payload sizes/depth before constructing untrusted collections.

`cargo test -p tf-domain --locked --offline` runs 15 constructor/invariant tests and
one compile-fail documentation test. T014 uses this crate for runtime semantic validation; three additional tests project 102 scalar-content cases, five logical
schema/field cases and catalogue identity examples from the shared T012 corpus
through the production constructors and back to the same carriers. They supplement
the existing full JSON-shape tests; the projection helpers are not shipped codecs.

T016 adds `diagnostic`: stable codes and exit statuses, immutable sanitized text,
source ranges, request context and bounded safe causes. It performs no I/O.
The [CLI guide](../transflow/README.md) documents redaction limits and projections.
