# DuckDB capability spike (T009)

This is an offline feasibility probe for the pinned DuckDB 1.5.5 and PyArrow
25.0.1, using the existing T003 engine environments. It does not implement the
application's canonical evaluator, scratchpad service or transform adapters.

```bash
cd ~/dev/transflow
target/qualification/py314/bin/python tools/qualification/duckdb/probe.py --output target/qualification/t009-py314.json
```

Prepare those environments using the [qualification setup](../../../docs/development/compatibility.md).
The existing native qualification command also runs this probe and preserves
`duckdb-capabilities.json` with its other reports. Its three-platform CI matrix runs
these tests after the changes are pushed. No specification checkout or new
package installation is required by the probe.

The controller starts a disposable helper with a small explicit environment,
private HOME/temp directory and a 90-second watchdog. Source environment variables
are not inherited. A successful report records normal helper reaping and directory
cleanup; forced watchdog expiry is not a tested application cancellation protocol.
Only successful runs write a report: check the command's exit status, and do not
treat an older report at a reused output path as evidence for a failed invocation.

## Measured boundaries

Sixteen tests exercise actual DuckDB/Parquet files and engine operations:

- Exact registered reads, configuration locking and disabled Python replacement scans.
- Nine engine-level denials for unregistered files, globbing, HTTP/S3 reads,
  outside writes, ATTACH and external extension installation/loading.
- Parsed SELECT/WITH, parameter values, joins, sorting and a window function;
  22 forbidden statement/function/table-access cases fail the prototype validator.
- Exact integer/decimal/count/null/NaN aggregates, unsigned integers, empty sets,
  Arrow/Parquet round trips and equivalent metrics on synthetic Polars,
  Arrow-backed pandas and trusted SQL materializations.
- Interruption after observable query startup; a one-million-row sort that
  physically spills, drains in 4,096-row batches and cleans temporary files;
  a zero spill budget fails with OutOfMemoryException.

The memory limit of 32 MB and spill allocation of 256 MB are explicit resource-test
settings. Normal helper setup leaves DuckDB's native memory limit unchanged.
That engine default is not a Transflow memory cap or a process RSS limit.

## Findings that must carry into implementation

DuckDB settings are defence in depth. The prototype applies an explicit parsed
AST allowlist before user SQL, and the canonical-check examples use fixed SQL
with bound values. A file in `allowed_paths` can still be overwritten by raw COPY
when temporary-file writing is disabled. A sacrificial fixture demonstrates this;
the scratchpad validator rejects the same statement before execution and preserves
the fixture bytes. Never expose the underlying connection as an unrestricted
scratchpad executor.

DuckDB automatically adds its configured spill directory to `allowed_directories`
when external access is disabled, even when application configuration supplied an
empty list. The test requires that this is the only directory entry. Give each
helper its own empty disposable directory; never use a workspace/source/data root
as that directory. The remaining allowlist consists of exact canonical input files.
The already-bundled core_functions, ICU, JSON and Parquet functionality suffices;
external auto-install/autoload, community/unsigned extensions and persistent
secrets are disabled. Extension metadata is inspected during trusted setup before
external access is disabled, because inspection can itself touch extension paths.

| Capability | Local measured result and required guard |
|---|---|
| Signed/unsigned 64-bit values and larger aggregate integers | Exact in tested Python integers and Arrow values; do not send through JSON floats. |
| Decimal precision up to 38 digits | Tested Parquet values and sums stay exact; overflowing sums raise OutOfRangeException. |
| Decimal average | DuckDB returns DOUBLE. This is unsuitable for an exact decimal metric without a separate exact implementation. |
| Decimal precision 40 | Engine returns DOUBLE with an incorrect observed value. Reject wider-than-38 decimal schemas before a canonical scan. |
| Naive nanosecond timestamps | Arrow and Parquet preserve nanoseconds; Python datetime retrieval loses submicrosecond precision. Use Arrow or explicit integer epoch values. |
| Timezone-aware nanosecond timestamps | DuckDB loses submicrosecond precision. The prototype rejects this schema before scanning. |
| UTC microsecond timestamps | Tested integer instants survive exactly. |
| Null and NaN | Remain distinguishable; canonical expressions must encode their specified treatment explicitly. |

The schema guard intentionally accepts only the tested primitive subset. It is
not the complete T036 logical-type contract. Unsupported exactness requirements
must fail visibly; no silent coercion, sampling or skipped-as-success checks.
The SQL validator covers a deliberately small pinned DuckDB AST subset. Unknown
constructs fail; T079 still needs the full reviewed grammar, typed bindings,
byte-budgeted result transport, deadlines and version leases. The prototype's
1,000-row fetch cap does not establish a bounded computation or byte allocation.

These conclusions combine local measurements with DuckDB's
[security controls](https://duckdb.org/docs/current/operations_manual/securing_duckdb/overview),
[configuration reference](https://duckdb.org/docs/current/configuration/overview),
[timestamp documentation](https://www.duckdb.org/docs/lts/sql/data_types/timestamp)
and [memory-management model](https://www.duckdb.org/2024/07/09/memory-management).
This is a trusted-local convenience, not a sandbox for hostile SQL or malformed files.
