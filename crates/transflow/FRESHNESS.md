# Frozen freshness and why services

T052 adds an internal read model. Public `why`/`plan` commands and UI adapters
remain T054 and later work; no transform is run by this service.

`tf-catalog::compute` derives versioned comparison evidence from the same
normalized carrier as the existing compute key. Adding evidence does not change
that key. It records code/declaration, environment, parameters, check and execution
policy fingerprints, exact alias-qualified input carriers, and captured file
hashes. File-level reasons can name a changed helper, SQL resource or lock file.
Whole-source hashing remains deliberately conservative. Secret values are never
included; only caller-supplied immutable secret version identifiers are hashed.

`transflow::cache::prepare` freezes this evidence before execution alongside the
publication contract. Additive runtime schema 7 stores it immutably by original
job. Cache adoption still points at the original publication and certificate.
Older jobs/imports without evidence remain unknown; no evidence is fabricated by
migrating them, inspecting them or retrying a later failed attempt.

`Reader::freshness_snapshot` reads heads, original publication contracts/check
results, and latest actual attempts in one SQLite read transaction. It returns
owned data with an event watermark and explicit workspace identity. Missing or
incompatible metadata is not currentness, a zero timestamp, or an empty certificate.
The snapshot is bounded at 100,000 heads/attempts and bounded check rows; exceeding
a bound fails explicitly instead of silently omitting data.

`transflow::why::inspect` runs on an owner-held blocking worker. It captures and
validates current source, takes one metadata snapshot, and leases exact selected
versions before verifying artifact bytes. All computation uses that captured graph
and those frozen heads, including fallback selection. Missing/corrupt existing
heads never cause fallback to another branch. No artifact scan or Python import
runs inside the SQLite transaction. Callers with already frozen facts can use
`transflow::why::explain` without further I/O.

The request supplies the same writer, parameter, clock and non-secret version
policies as computation planning. Omitted policies produce unknown comparison
facts. Unsupported foreign/provider contexts and pending registrations stay
unknown; an older source definition never becomes an executable fallback. Retained
heads absent from the selected source stay inspectable with an explicit reason.
Actual source imports still run trusted local Python, as in validation/planning.

`tf-plan::freshness::evaluate` walks the validated dependency order once, memoizes
ancestor causes and retains a deterministic shortest path for each root cause.
It keeps these facts separate:

- Materialization and exact artifact availability.
- Direct input/source-refresh freshness and direct logic freshness.
- Inherited ancestor freshness, even if the immediate parent version is unchanged.
- Latest actual attempt, independent of cached head adoption.
- Original output quality and contextual consumer input results.

Off-branch, fallback and foreign boundaries cannot borrow the selected branch's
producer logic. Unassessable boundary freshness remains unknown; exact input
identity comparisons are still separate facts. Source TTL uses the original
publication time. `cache="never"` cannot receive deterministic currentness merely
because its visible fingerprints match. Causal path storage has an explicit
one-million-cell bound; requests exceeding it fail rather than truncate reasons.

Each structured reason retains its code, before/after evidence, alias/path and
relevant effective policy. `Reason::human` renders those same fields; it does not
implement a second freshness heuristic. `CACHE_MATCH` is an explanatory match,
not permission to skip the guarded cache lookup and artifact verification service.

Qualification includes pure diamond/source/unknown/boundary cases, real captured
source with SQLite/Parquet publication and failed later attempts, immutable
comparison records, stable frozen snapshots, and installed-worker inspections
that neither execute producers nor register pending outputs. Test-only SQLx in
the composition crate reuses the already-qualified version and shared publication
fixture; production dependency boundaries are unchanged. See the repository
verification report for actual results and CI handoff status.
