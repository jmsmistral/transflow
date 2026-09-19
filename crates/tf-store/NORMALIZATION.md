# Logical schema and value normalization

T036 provides the shared Rust normalization service used by import staging and
future artifact/execution services. It projects qualified Arrow types into the
closed `LogicalSchemaV1` and computes the existing canonical schema fingerprint.
It validates actual decoded values before a copied import can be retained.
It does not publish artifacts, evaluate expectations or implement transform workers.

Supported logical types are booleans, all signed/unsigned integer widths, binary32
and binary64 floats, UTF-8 strings, binary, Date32 dates, decimal128, timestamps,
lists and structs. Field order, nullability, decimal precision/scale, timestamp
unit and original timezone remain explicit. Field metadata is retained; empty
metadata normalizes to absence. String/binary offset widths and view layouts
normalize to their logical type. List/large-list physical child names normalize
to `element`; struct and top-level field names remain unchanged. There are no
value casts or file rewrites. These choices follow the separation of parameters
and physical layouts in the [Arrow format](https://arrow.apache.org/docs/format/Columnar.html).

Duplicate names, extension/object types, dictionaries, maps, duration/time values,
Date64, decimal256 and untyped-null columns require explicit conversion. Root
engine metadata has no portable schema-level carrier and requires explicit removal;
this prevents pandas/index metadata from silently changing the table contract.
Schema metadata has a 1 MiB budget, at most 4,096 fields and 24 nested levels.
These metadata bounds are not a default dataframe memory cap.

`validate_batch` checks schema agreement, visible nullability, date/timestamp
calendar range and decimal coefficients without serializing a dataframe. Children
masked by null parents are absent logically. `cell` encodes one bounded typed
value (64 KiB encoded JSON; bounded nodes/depth), preserving integer strings,
fixed-scale decimals, null/NaN/infinities and signed zero. Aware timestamps use
UTC calendar text plus the original timezone; naive timestamps have no `Z` and
are never silently labelled UTC. Dates/timestamps use the wire range 0001–9999.

`inspect_parquet` reads through a caller-owned stable file descriptor, checks a
bounded footer, decodes all rows in batches, validates values and returns logical
schema plus exact row count. A valid zero-row file retains its schema. The caller
still owns containment, file hash/identity checks, leases and publication.

## Adapter capabilities

`NormalizedSchema::require` is a schema preflight, separate from representation.
It recursively reports unsupported fields. It is not a guarantee that an arbitrary
expression, installed engine or timezone database can execute a plan; worker
capability/environment checks remain required.

| Boundary | Qualified scope and required errors |
|---|---|
| Parquet storage | Portable types above; decimal scale 0 through precision; timestamps ms/us/ns. Second-resolution timestamps and negative/excess decimal scales require explicit casts. |
| Polars | Same initial type preflight; actual engine round trips qualify primitive widths, lists/structs, naive ns and named-zone ns. Objects are rejected, never stringified. |
| DuckDB validation | Compatible portable types; naive ns and UTC ms/us. Aware ns and non-UTC timezone labels fail before the engine can lose precision or change metadata. UTC session configuration is required. |
| pandas / DuckDB SQL transforms | Explicitly unavailable until their later adapter tasks. Dependency probes do not enable execution. |

## Verification

```bash
cargo test --locked --offline -p tf-store --test normalization --test imports
cargo build --locked --offline -p tf-store --example normalization_probe
target/qualification/py314/bin/python -I -B python/tools/check_normalization.py target/debug/examples/normalization_probe
```

The repository-owned fixture has three rows with integer extrema, null/NaN,
infinities, signed zero, precision-38 decimals, negative epoch ticks, ms/us/ns,
UTC and Europe/London labels, strings, binary, dates and nested values. Actual
PyArrow and Polars rewrites are inspected by production Rust normalization and
compared to original typed values and fingerprints; empty Polars output keeps
schema. The compatible DuckDB subset is compared identically with its session
explicitly set to UTC. Object and duration failures are also tested. The probe
runs in the six-job native qualification matrix on the locked engines.
