# Branch-scoped cache reuse (T051)

`transflow::cache::prepare` loads the retained accepted plan and source capture,
revalidates its graph, resolves each boundary and completed in-build parent, and
finalizes T050 computation/check fingerprints. Pending parents produce no key.
A child reads its parent's successful or cached result from that build, never a
later branch head. Force, source refresh and `cache="never"` require execution.
The trusted check planner supplies complete normalized check obligations; this
service verifies their phase/alias/severity coverage. Canonical check compilation
and evaluation remain T059–T064. Public build dispatch remains T067.

`tf-store::cache` searches only versions materialized in the output branch, with
matching computation and check/evaluator fingerprints. The current matching head
wins, then newest publication with a stable version-ID tie-break. Lookup validates
original successful attempt/contract, check subjects/outcomes and published result
links. It atomically acquires a build lease before any filesystem read. No match
is an ordinary miss; inconsistent evidence and missing/corrupt selected bytes are
explicit errors, never a search for a more convenient older candidate.

`tf-exec::cache::reuse` keeps the real runtime lock throughout blocking full-byte
Parquet/manifest verification and guarded adoption. It releases the candidate
lease on success and ordinary failure, preserves original errors, and fails when
a scan outlives its lease. If its awaiting future is dropped, the blocking task
retains ownership until it actually finishes. No transaction spans artifact I/O.

One IMMEDIATE transaction rechecks owner, complete write reservations, accepted
plan/source/contract, no-attempt job state, cancellation, live dataset/branch,
expected head generation, lease, manifest and original check evidence. It records
CACHED and schema-6 cached_jobs/validation_reuse associations plus an audit row.
No attempt, version, materialization or new check evaluation is created. Original
PASS/WARN outcomes and evaluation/publication timestamps remain unchanged.
Changing WARN to FAIL or normalization/evaluator semantics invalidates reuse.

Adopting an older matching version advances the head generation and atomically
writes `dataset.head_changed`, head history and the existing durable publication
outbox. An already-current version leaves the head and event stream unchanged.
Neither emits a new `dataset.published`. Retrying the same accepted job returns
its durable receipt under the original fence; startup preserves committed cache
evidence. Write reservations remain until build termination. Cached results of
active builds are retention roots; exact child provenance remains per alias.

Schema 6 is additive and preserves all previous migration bytes. Physical object
deduplication remains independent: a forced materialization still creates a new
version/attempt even when its bytes match an older object. Cross-branch compute
adoption, partial check-only revalidation, public execution and foreign dispatch
are not supplied by this internal service.
