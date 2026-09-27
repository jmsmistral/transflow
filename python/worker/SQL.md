# Restricted SQL helper

T079 provides the production AST policy and a private, isolated one-query helper.
It does not expose a browser/HTTP scratchpad yet. T080 connects query IDs, leases,
timeouts, cancellation and result delivery to the coordinator; it must also check
that the qualified Arrow dependency is available before advertising the service.

## Query policy

The pinned DuckDB 1.5.5 parser must recognize exactly one SELECT/WITH statement.
The helper walks its serialized AST before executing the unchanged SQL on the
same fresh connection. Allowed relations are explicitly bound dataset-version
views, local CTEs, joins and subqueries. Grouping, ordering, filtering, CASE, scalar
casts and reviewed window/aggregate functions are supported. Parameters travel
separately as strings, booleans or null; use explicit SQL casts for numeric/date
parameters to avoid lossy JSON numbers.

The reviewed function list is in
[sql_policy.py](src/transflow_worker/sql_policy.py). Unknown functions, nodes,
operators and types fail closed. Writes, multiple statements, configuration,
macros, extension loading, table functions, file/URL relations and catalog/schema
qualified names are refused. Recursive CTEs, set operations, sampling, dynamic
column expressions and custom/nested casts are outside the initial subset.
SQL text is capped at 16 KiB and parsed structures have depth/node/size guards.

Fresh connections expose only canonical, digest-verified manifest files. The
helper rejects input globs, links, case-colliding columns and known lossy input
conversions. The initial input subset is scalar, including decimals up to precision
38 and naive nanosecond timestamps. Aware nanoseconds, non-UTC timezone labels,
nested inputs and unsupported decimal scales fail explicitly. SQL expressions
still have DuckDB semantics: for example, decimal AVG produces a floating result.

External access, extension installation/autoload, community/unsigned extensions,
persistent secrets and Python replacement scans are disabled. Configuration is
locked after trusted view creation. Only exact input paths and the engine's one
fresh private spill directory are accessible. The fixed helper spill ceiling is
256 MiB; no Transflow memory cap is installed. These are defence-in-depth controls
for trusted local use, not a hostile-SQL sandbox. Engine permissions alone do not
prevent every write, so bypassing the AST validator is forbidden.

## Private boundary and ownership

The installed module entry point is
`python -I -B -m transflow_worker.scratchpad REQUEST_FILE`. It requires a private
request file and a fresh private result directory, validates the shared
`ScratchpadRequestV1` contract, clears inherited environment variables before
loading engines, and never imports workspace code. A launcher must additionally
pass a clean environment and supervise the process. This is an internal building
block, not an end-user command or an independently supervised query service.

The caller must resolve and verify exact local/provider version bindings and hold
renewable leases until helper exit and result acceptance. The helper does not
resolve `latest`, contact a provider, acquire leases, mutate heads or publish
datasets. It scans provider paths directly; no input copy or replica is created.
Bindings retain alias, workspace, dataset, version and artifact identity.

Results use Arrow IPC (`result.arrow`) to preserve integer/decimal/timestamp
precision, with a closed `ScratchpadResultV1` metadata file (`result.json`). Reads
use batches of 128 and retain at most 1,000 rows and 2 MiB of serialized output;
truncation is explicit. A batch that would exceed the byte limit is omitted, so
fewer than 1,000 rows, including zero, can be returned. Output limits do not bound
query computation or decoded-cell memory. T080 supplies the interactive deadline,
supervised cancellation, public typed result conversion and lifecycle integration.

Success requires unchanged input file identities after execution. Failure removes
partial result files; normal exit removes spill files. The caller must remove all
temporary state after forced termination and eventually delete accepted results.
Rows/control payloads never go to stdout; stderr contains fixed safe diagnostics,
without SQL, parameters, paths or engine exception text.

## Verification

Pure policy tests run in the ordinary Python gate. Shared binding/request/result
fixtures run in Rust, Python and TypeScript. Native qualification installs the
actual wheel plus pinned DuckDB/PyArrow into a disposable environment offline,
then checks analytical queries, AST and engine denials, provider-byte preservation,
precision, caps, corrupted inputs, safe errors and process/spill cleanup:

```bash
python -I -B python/tools/check_scratchpad.py /absolute/path/to/transflow.whl --output /tmp/scratchpad.json
```

The native aggregate runs this automatically using its freshly built wheel and
prepared `target/qualification/wheelhouse`. PyArrow 25.0.1 is required for this
helper; this task does not change existing workspace environment locks or add
automatic dependency installation during queries.
