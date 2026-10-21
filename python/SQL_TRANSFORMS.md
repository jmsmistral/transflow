# Managed SQL transforms

Keep the declaration and query in one Python file:

```python
from transflow import Input, Output, sql_transform


@sql_transform(
    deliveries=Input("curated/delivery"),
    output=Output("analytics/region_summary"),
)
def region_summary(deliveries, *, output):
    return """
        SELECT region, COUNT(*) AS order_count
        FROM deliveries
        GROUP BY region
    """
```

`deliveries` in SQL is the registered input alias, not a Python variable or catalogue
path. A static query can use `def region_summary():` instead: every declared data
alias remains available in SQL. There is no `sql_transform_meta` decorator. Declared
Python arguments always receive [enriched handles](TRANSFORM_FACTORIES.md).

The function runs once during its supervised build, after required input checks.
Discovery registers definitions without invoking their query functions. The return
must be a nonempty SQL string, at most 512 KiB. Python may construct query shape
using frozen metadata and `ctx.params`; dependencies must all be declared before
planning. Trusted imports and hidden I/O are not a purity sandbox. Use
`cache="never"` for computations whose external state/time/randomness is not captured.

Use typed parameters and DuckDB value binding for values:

```python
from transflow import Parameter


@sql_transform(
    deliveries=Input("curated/delivery"),
    output=Output("analytics/large_deliveries"),
    params={"minimum": Parameter({"type": "i64"}, {"type": "i64", "value": "100"})},
)
def large_deliveries(*, ctx):
    return "SELECT * FROM deliveries WHERE amount >= $minimum"
```

Declared parameters may also affect Python query construction. Only placeholders
present in the returned statement are passed to DuckDB; undeclared placeholders
fail. Never interpolate untrusted values into SQL. Alias identifiers and values
are separate concepts; quote identifiers if they are reserved words.

The original paired-file form remains supported:

```python
summary = sql_transform(
    "summary.sql",
    deliveries=Input("curated/delivery"),
    output=Output("analytics/region_summary"),
)
```

The `.sql` path is relative to its registration module, inside captured roots, and
cannot be excluded or symlinked. File registrations remain immutable descriptors;
function decorators preserve ordinary direct Python calls. Existing captured files
up to the source-resource limit remain executable; generated-SQL inspection retains
queries up to 512 KiB.

Add qualified DuckDB 1.5.5 and PyArrow 25.0.1 to workspace requirements, then run
`env lock` and `env sync` explicitly. Builds install nothing. Exact input aliases
become managed views. Validation-only inputs participate in checks and planning,
but are not query views or function arguments.

The query must be one table-producing statement. The managed connection stays
open through its single Parquet sink. Reads are limited to exact pinned input
files. Python replacement scans, automatic extension installation/loading and
unsigned/community extensions are disabled. The browser scratchpad remains a
separate, narrower query interface.

## Inspect authored code and generated SQL

The lineage Code panel keeps Latest saved version, Data publish version and Diff.
For SQL it adds Authored code and Generated SQL beside the dataset name, with
matching compact version controls. Generated transforms show an italic `d` before
the lineage function glyph and a Dynamically generated code label in Code.
Imported factories offer Factory
definition and Registration source views. Diff compares authored captured code.
Generated SQL shows actual retained execution evidence, including SQL whose parse
or execution failed. Latest saved mode selects the latest job's attempt; Data
publish version selects the attempt that produced that exact publication. Cached
reuse refers to its original producing attempt. A generation failure never falls
back silently to older successful SQL. Evidence retains its attempt and source
in the API; the code pane omits this metadata row. Saved code may have changed
since execution. Reads do not invoke query factories.
The SQL display removes surrounding blank lines and the shared indentation margin,
preserving relative alignment. This presentation does not alter retained or executed SQL.
Bound values remain in the separate attempt parameters, not interpolated into SQL.
SQL evidence expires with retained attempt logs and may be explicitly unavailable.

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
