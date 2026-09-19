# Candidate reconciliation (T028)

`CandidateCatalog::prepare` consumes the complete validated discovery result and
its explicitly captured registry/source context. It performs no filesystem,
process, random-number or database work. The result/source/catalogue identities
must match before reconciliation begins. Callers supply the registry parsed from
that capture, not a subsequently edited working copy.

All outputs are collected before any input is resolved. Existing IDs and kinds
remain unchanged; only new local literal paths can become pending outputs. Pending
identities are `CandidateIdentity::Pending(DatasetPath)`, deliberately distinct from
registered owner-qualified UUIDs. Input-only typos, unknown IDs, tombstones, foreign
outputs and kind changes fail without creating placeholders.

The candidate preserves every input alias and its branch, stop-fallback, role and
check metadata. Existing rename aliases resolve exactly. Foreign registrations
remain read boundaries. Bound C references must match the catalogue fingerprint,
owner and dataset at their captured path. One producing module/output is required;
collisions report both source locations.

Cycle detection covers the entire local candidate, including validation-only,
repeated-alias and off-branch edges. Diagnostics retain a closed cycle path and its
source locations. Traversal is iterative, not recursion limited. Topological output
uses stable identities/pending paths to break ties among ready producers. Helpers
and registered identities without a currently discovered producer remain valid.

Only a successfully prepared candidate exposes `propose(allocator)`. The explicit
draft/mutation caller allocates IDs once, in pending-path order. A proposal retains
those exact assignments, expected-old byte digest, source/catalogue context and
complete deterministic replacement bytes. It is not yet a committed registry.
Future draft persistence must save the assignments instead of invoking the allocator
again. Validation has no allocation callback and never obtains a durable identity.

Rendering preserves all existing datasets, aliases, tombstones and external
registration policies, including fallback order. Complete parsing revalidates
collisions before returning bytes. TOML comments/formatting are presentation;
replacement has deterministic formatting. The SDK projection has one canonical path per identity and explicit local/foreign
alias mappings. T030 binds these aliases into the same immutable catalogue
fingerprint. Exact strings and bound alias references resolve consistently.

T029 owns guarded writes and recovery. T032 extends structural checks to complete
AST/engine/schema capabilities, and T033 integrates preparation services. Public
validate/plan/build CLI dispatch remains later work; this API does not execute
checks, import provider code or publish data.
