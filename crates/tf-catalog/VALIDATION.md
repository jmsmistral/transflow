# Structural validation

`tf_catalog::validation::validate` accepts one complete captured workspace context
and returns a `ValidatedGraph` only after all structural checks pass. There is no
selected-target, pin, force or skip-validation argument. It performs no filesystem
writes, UUID allocation, producer invocation, provider access or dataset reads.

The preparation caller supplies the captured configuration and registry, verified
source ID/digest, the complete pre-import Python `ModuleIndex` (including helpers
and namespaces), the authenticated discovery response, and the verified installed
environment and matched SDK/check semantic versions. The caller must verify
captured bytes and environment drift. Supplying a caller-filtered module index is
outside this contract: module enumeration and signature/NFKC validation remain
owned by the captured Python index/SDK, not reconstructed with a second Python
parser in Rust. Import errors cannot be converted into a successful discovery
response. The service checks complete import coverage and definition locations.

Validation reparses configuration, verifies ownership/context, rejects unsupported
engines/contracts, reconciles all outputs before inputs, and checks the complete
local graph including repeated aliases, validation-only and named-branch edges.
It validates source/transform policies, public non-reserved input identifiers,
typed parameter defaults, unique check IDs, expectation arity/columns and inherited
null/sample policies. Unicode identifiers use the already-locked `unicode-ident`
1.0.26 library; the dependency graph adds no package version. The [T059 AST contract](../../python/EXPECTATIONS.md) adds nested typed composition,
explicit comparisons and declared input references; unknown operations fail, including
under WARN. The check semantic version changes to invalidate earlier certificates.
The legacy two-operation seed normalizes to AST-v1 before hashing. T060–T062
extend the remaining DSL and evaluator. Sample row settings control later diagnostics, never
sampling-based acceptance.

Declared schemas permit early rejection of impossible column references. They
still require verification against exact materialized bytes. A foreign binding,
a named-branch input or an unavailable future schema produces explicit deferred
schema obligations. Every check also retains a deferred data/schema obligation;
there is no fabricated data-check PASS. Polars return validation and actual
schema/engine exactness remain execution/evaluator boundaries (T035/T036/T061),
including the qualified DuckDB decimal/timestamp restrictions.

A failure carries safe explanatory text, declaration line anchors and a complete
closed cycle of dataset paths. T016's renderer receives sanitized source ranges
and affected references. Its display limit is explicit; the structured cycle is
not truncated. Anchors identify the first character of the known declaration
line; this service does not read source bytes to invent precise expression spans.

Certificates bind source ID/content, registry bytes/catalogue fingerprint,
configuration bytes, complete module index/discovery, installed environment,
SDK/check versions and normalized candidate/check semantics. Their digest domain
is the existing Compute hash with a versioned validation descriptor. `evidence()`
returns a detached internal inspection descriptor, not a public wire/persistence
format. `matches` requires the exact context; edited comments conservatively
invalidate reuse. This evidence is not a data quality/publication certificate.

`ValidationState` retains the last successful graph for an explicitly stale display.
After a failed submission, `current_for` refuses even the previously valid context
until successful validation is submitted again. A later selected/forced build must
use this shared service for the whole requested capture. The public preparation,
validate/catalogue CLI and durable graph/certificate persistence remain T033 and
later composition tasks; no new CLI capability is advertised here.

Tests exercise whole-graph failure/preserved display, concrete cycle diagnostics,
all certificate inputs, configuration/import coverage, invalid declarations,
known versus deferred schemas and foreign/named-branch boundaries. A shared
synthetic fixture is reproduced by a real installed discovery worker over its
private socket and consumed by Rust validation. Its producer bodies raise if
invoked; discovery and validation succeed without executing them or allocating IDs.
