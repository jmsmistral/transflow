# Polars execution adapter (T058)

The private `execute` worker operation materializes one captured Polars producer.
It is a backend service used by future runtime dispatch, not a public `transflow
build` command. Polars 1.44.2 is qualified on the pinned engine environment. The
SDK still imports without Polars; execution reports a missing environment instead
of installing dependencies. Pandas and SQL transform adapters remain later work.

## Accepted execution and immutable inputs

`PolarsExecutionRequestV1` carries the captured source/catalogue/environment
identity, exact discovered producer declaration, ordered alias-qualified version
and artifact bindings, resolved context, writer settings and thread count. It is
bounded by the supervisor's 1 MiB private request limit. `polars.execute.v1` is a
negotiated capability using existing protocol 1.0 execute/artifact-ready frames.
Unknown shapes or capabilities fail closed; generated Python/TypeScript/schema
exports and conformance cases share the authored contract.

The Rust `tf_exec::polars::run` caller holds runtime ownership, input leases and
admission/timer policy and verifies the environment before dispatch. Required input
expectations must already have passed their separate validation service. The
adapter checks source and data integrity; it does not implement or bypass the
later T061/T062 expectation evaluator.

Each input must match the caller's verified immutable artifact, including manifest,
digest and exact local object directory. Rust re-verifies it before launch. Python
checks manifest/schema fingerprints, file hashes, lengths, regular-file identity
and supported types before invocation. Lazy scans use the ordered explicit file
list with globbing and hive inference disabled. No latest-head lookup, directory
scan, branch fallback, casts, missing-column insertion or foreign producer execution
occurs. Repeated dataset aliases remain separate bindings; validation-only aliases
are verified but are not passed as function arguments.

The supervisor installs all engine thread environment limits before Python imports.
The adapter additionally checks Polars' actual pool size against the request. The
captured-source loader verifies bytes and reimports the immutable catalogue context;
the producer must exactly match the accepted declaration. Helpers use the same
captured source mechanism as discovery.

## Single materialization and independent candidate checks

The function is called once. Its result must be one actual `pl.DataFrame` or
`pl.LazyFrame`; DataFrame returns become lazy sources for the same sink. Unsupported
objects, generators and multiple tables are errors. Schema inspection uses
`collect_schema`, not an eager collection of the output. The supported logical
projection includes integer widths, floats, null values in typed columns, decimals,
strings/binary, dates, timestamps, lists and structs. Object, categorical/enum,
untyped-null, duration/time and fixed-array types require explicit conversion.

The adapter invokes `sink_parquet(engine="streaming")` once into a create-new
private `part-00000.parquet`. Compression and row-group size are explicit. Zero-row
outputs retain their schema. Writers close and flush before a manifest is emitted.
The adapter reopens only these staged bytes for schema, hash and a scalar row-count
query; it never reruns the producer or its LazyFrame to validate output. Polars
writes nullable fields; a declared schema must match the physical logical schema
exactly, including nullability. There are no implicit casts or invented non-null
certificates. Non-null expectations remain separate checks.

Rust treats the worker manifest as untrusted evidence. It copies the exact staged
file into the existing private candidate store, decodes every Parquet batch through
T036 normalization, checks supported values/schema, recomputes hashes/counts and
compares the complete manifest and artifact digest. This byte copy is not another
query materialization. A discrepancy drops the candidate. A successful return is
an invisible closed `Candidate`, ready for output checks on those bytes and the
separate guarded publication service. The adapter has no database mutation API.

The execution context supplies immutable resolved parameters, build/job/attempt
IDs, an aware logical evaluation time, optional explicit random seed and stderr
logging. Typed parameter conversion preserves integer/decimal precision and nested
immutability. Submicrosecond timestamp parameters are rejected because Python
`datetime` cannot represent them; this does not restrict timestamp columns.
Secret resolution is deliberately unavailable until the source execution service
provides it. Cancellation uses the existing owned process group and coordinator
heartbeat/disconnect cleanup; the context does not poll a runtime database.

## Verification and boundaries

The [native runner](tools/check_polars_execution.py) loads an actual built wheel in
an isolated qualified Python worker, supervised by the Rust service. Twenty-two
cases cover exact ordered files, DataFrame/LazyFrame/empty output, one invocation
and one instrumented lazy execution, repeated and validation-only aliases, context,
thread policy, primitive/nested/precision round trips, unsupported returns/types,
changed source/bindings, retained output files, exceptions and streaming. Every
case retains a real pre-existing SQLite publication and proves its head, version
and event unchanged. Existing publication/cancellation tests cover subsequent
visibility and cancellation ordering.

The streaming fixture scans eight million rows and sinks sixteen integer columns
(1.024 GB logical values). Its measured worker peak RSS must be smaller than the
logical output. This qualifies a supported query shape; sorting, joins, Python UDFs,
eager user code and other plans can still require substantial memory. No default
memory cap or spill guarantee is introduced. Rust candidate checking and file
copies have separate resource costs; the reported RSS is the Python worker's.

`tools/qualification/check.sh` builds the local wheel and runs this suite on the
existing macOS arm64/Linux x86-64/Linux arm64 matrix with Python 3.14.7. Local
[evidence](../docs/development/evidence/t058-macos-arm64.json) records only executed
platforms. Durable runtime dispatch, public build/serve, checks and publication
composition remain T061–T067; source refresh/secrets and other adapters remain
later work. No expectation or dataset publication is claimed by `artifact_ready`.
