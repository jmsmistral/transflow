# Managed SQL transforms

Register a SQL producer in a Python module and put its query beside that module.
Each module declares one producer and one output. Registration reads no SQL file
and imports no data engine.

```python
from transflow import Input, Output, Parameter, sql_transform

orders = sql_transform(
    "orders.sql",
    rows=Input("raw/orders"),
    output=Output("curated/orders"),
    params={"minimum": Parameter({"type": "i64"}, {"type": "i64", "value": "10"})},
)
```

```sql
SELECT * FROM "rows" WHERE amount >= $minimum
```

Add the qualified DuckDB 1.5.5 and PyArrow 25.0.1 dependencies to the workspace's
requirements, then explicitly run `env lock` and `env sync`. Builds install
nothing. The registration is an immutable descriptor, rather than a callable
Python transform. Named data inputs become quoted views; validation-only inputs
are available to checks and do not become SQL views. Parameter values are bound
separately. SQL that uses current time or randomness should declare `cache="never"`.

The query must be one table-producing statement. The managed connection stays
open through its single Parquet sink. File access is restricted to exact pinned
inputs and the output file. Python replacement scans, automatic extension
installation/loading and unsigned/community extensions are disabled. Optional
extensions are not enabled by this adapter. The separate browser scratchpad has
its own narrower query policy.

The `.sql` path must be relative to the registration module, inside captured
source roots, and not excluded or symlinked. SQL bytes and helper/lock files are
captured and fingerprinted. Changing SQL invalidates reuse; replay reads the
retained SQL, without changing live files. Code view displays the producing SQL
file and can compare it with the latest retained SQL source.

## Exact type profile

The managed adapter preserves booleans, signed/unsigned integers through 64 bits,
Float32/Float64, UTF-8 strings, binary, dates, Decimal128 with nonnegative scale,
naive millisecond/microsecond/nanosecond timestamps, UTC microseconds, and qualified
lists/structs. Input projections restore declared types before execution, including
DuckDB's millisecond Parquet-reader mapping. The inferred SQL output schema must
match any explicit `Output(schema=...)`; no output casts repair a mismatch.

Timezone-aware nanoseconds, non-UTC timezone metadata, UTC milliseconds, wider
decimals, unsupported Arrow types, case-colliding names, metadata or nullability
that the SQL view cannot retain, and glob-bearing artifact filenames fail
explicitly. Normalize unsupported inputs in an upstream producer before SQL.
Python datetime parameters cannot carry submicrosecond timestamps; use an explicit
supported parameter representation or author a typed SQL expression.

## Review fixture

See [SQL and checks review](../examples/sql-checks/README.md) for a mixed-engine
workspace with branch-qualified validation aliases, checks, cached history and
paused schedules. Native qualification includes exact logical-value comparisons
and canonical checks on both managed adapters' output bytes.
