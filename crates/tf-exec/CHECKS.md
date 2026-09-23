# Canonical expectation evaluation (T061)

`checks::run` evaluates G1 checks over a borrowed `VerifiedArtifact` or closed
`Candidate`. It never imports a producer, invokes a transform, changes heads or
publishes. Callers retain exact version leases, candidate ownership, environment
verification and the producer's resource reservation until helper cleanup completes.
T062 integrates input/output gates below; T063 adds reusable certificates, complete
failed-attempt evidence persistence and bounded samples.
Public build execution remains later orchestration work.

Rust validates resolved check policies and the actual normalized schema, compiles
the typed AST, and replaces caller query/path fields with its own compiled plan and
verified subject. The private request is not an author SQL interface. Identifiers
are quoted, literals are bound with exact typed casts, and only exact manifest
files are scanned. Canonical staging filenames cannot contain glob syntax. A
candidate's digest is the same artifact identity subsequently checked at install.
The service strictly verifies subject bytes before and after helper execution.

Compatible row expressions share aggregate scans in batches of 32 predicates.
False and unknown counts remain separate until the complete predicate's root
policy is applied. Child metrics retain stable `$`/child-index paths. Primary keys
use exact tuple GROUP BY, excluding null-key rows, and count every duplicate-group
row. Row counts, exists and has_type conditions retain their distinct dataset
semantics. Dataset composition and query failures cannot hide errors through
short-circuiting. NaN comparisons are explicitly normalized instead of inheriting
DuckDB's ordering; decimal/u64/nanosecond values never pass through Python float or
datetime. Only bounded aggregate cells return to Rust, never dataframe rows.

The helper requires `duckdb.checks.v1` alongside the AST/core capabilities and
DuckDB 1.5.5. It starts a fresh in-memory connection without source credentials or
Python replacement scans. Extension installation/autoload, community/unsigned
extensions, persistent secrets and external access are disabled. Exact input paths
and one disposable spill directory are allowlisted; configuration is then locked.
These settings are not a general hostile-SQL sandbox: the helper executes only
application-compiled requests. The service discards caller-supplied SQL.

An attempt-wide input/output validation Budget is mandatory and reused across
helpers/queries. Setup, integrity checks and evaluation consume that phase budget;
queue time and supervised termination grace do not. Explicit zero disables the
phase deadline; cancellation still works. Threads follow the producer reservation.
`memory_bytes=null` preserves DuckDB's native default and adds no Transflow memory
cap. An explicit memory value controls DuckDB's buffer budget, not total process
RSS. `spill_bytes` is an explicit resolved temporary-disk budget; tests use small
values without establishing application defaults. The parent owns spill storage
and removes it after normal completion, cancellation or forced worker cleanup.

Results bind request/attempt, artifact, input version or candidate, consumer,
binding/phase, check definition digest, evaluator/semantics, elapsed time, severity,
exactness and observed/expected child metrics. Errors retain safe codes without
engine text or cell values. Dataset conditions without row attribution report
`failed_rows=null`; errors discard partial metrics and cannot become PASS under
WARN. Result frames require completed successful process termination, matching
identity/digest, closed schemas and bounded result files. A process/transport/
result-integrity failure invalidates all results from that helper.

Known lossy schemas fail before scanning: decimals outside the qualified scale/
precision range, timezone-aware nanoseconds, unsupported timezone/unit mappings,
and case-colliding field names under DuckDB's identifier rules. Floating/binary/
nested primary keys and incompatible operands are errors. G3 input-metric references
remain unsupported. Samples remain absent; nonzero sample requests return an
explicit error until T063 instead of silently ignoring the requested policy.

Verification uses the actual Rust compiler, artifact service, packaged helper and
DuckDB through `python/tools/check_expectations.py`, integrated into native
qualification. Synthetic Arrow, Polars, Arrow-backed pandas and trusted SQL
materializations yield identical outcomes; these are fixture writers, not new
transform adapters. Coverage includes complete truth tables, null/NaN behavior,
quoted identifiers and bound injection-like values, exact wide values, empty
subjects, candidates, WARN errors, active-query deadlines, cancellation, physical
key-group spill, zero-disk failure, restricted paths and cleanup.

## Single-attempt lifecycle (T062)

`lifecycle::run` accepts the complete frozen publication contract, ordered verified
inputs, resolved declarations and one admitted producer reservation. It rejects
missing obligations, changed versions/aliases or unresolved policies before any
producer runs. Independent input aliases are evaluated before deciding the input
gate. Every ERROR and every FAIL violation blocks; WARN data violations permit
continuation. Nonexecuted output checks have an explicit SKIPPED reason.

The same reservation supplies one worker permit at a time, with a shared Budget
for all helpers in each validation phase and a separate transform phase. A phase
callback lets the owning coordinator persist boundaries with
`Store::advance_gate_phase`; its short transactions recheck exact accepted attempt,
reservation and cancellation authority. The output boundary follows closed
materialization, including both RUNNING-to-MATERIALIZING and
MATERIALIZING-to-VALIDATING_OUTPUTS transitions. Live per-worker phase projection
and terminal job/build dispatch remain part of T064.

Only successful gates return an `Approved` candidate. This private token binds its
attempt, complete contract, exact candidate and evaluator results. The consuming
`lifecycle::publish` verifies those identities against the legal domain intent,
persists the minimal exact gate rows, and uses the existing journal/install/commit
protocol. It uses the owned candidate directly: no second function invocation,
sink or staging copy. Cancellation and head-generation checks still apply at the
SQLite visibility boundary. No worker obtains publication authority.

Input check identities are scoped to the consumer's exact alias, version and
artifact. Identical declaration digests on two aliases or on input and output get
separate contract obligations. Publication links results to the consumer's new
version only; it never changes a provider's quality certificate. WARN results stay
VIOLATION with their severity and exact metrics; publication never rewrites PASS.
The full envelope and bounded logs remain in the lifecycle completion.

Output failures and precommit refusals retain sealed, strictly verified candidate
objects without a version/head reference. The completion exposes the candidate
identity and manifest for diagnostics, independently of normal dataset reads.
Retention failures remain visible alongside the original refusal. These objects
are not reusable certificates or successful dataset versions. T063 will provide
complete durable failure/certificate/sample records; T064 will connect these
services to graph dispatch, held input leases and terminal job/build persistence.
Environment verification, runtime ownership and leases remain caller obligations.

Native `python/tools/check_lifecycle.py` runs the packaged workers and real SQLite
publication across PASS/FAIL/WARN/ERROR, repeated aliases, absent obligations,
cancellation, conflicts, candidate tampering and producer failures. It checks
function counts, preserved last-good version references, invisible retained bytes,
exact output artifact identity and warnings linked after successful publication.
