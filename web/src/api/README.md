# API contract boundary

Rust owns the API schemas and generated TypeScript contracts. `client.ts` validates
responses with the browser-safe `validate.ts` engine, also used by the shared
conformance tests. Browser requests use same-origin sessions, in-memory CSRF and
the read-only POST facade. Credentials never enter browser storage or logs.

`EventCursor` keeps lossless sequence and UUID resume state. `stream.ts` bounds SSE
frames, deduplicates facts and refetches authoritative models at checkpoints.
Resync gaps are acknowledged only after refetch; interrupted bodies are reported
as disconnected separately from invalid schema responses.

`../workspace.ts` owns cancellation/selection epochs and context equality. New
inspectors must use its guarded read boundary and keep source/rows bound to the
selected exact version. Do not infer domain semantics or mutate build state from
event notifications. Graph interactions and layout workers remain T083.

See the [HTTP guide](../../../crates/tf-api/README.md) for authentication and replay.

Lineage responses use `ApiLineageNodeV1`: `parent_count` and `child_count`
come from the complete captured local graph, not the returned page's edges.
This lets a depth-zero node offer valid expansion without per-node requests.
Foreign nodes have no upstream provider expansion. CLI `GraphNodeV1` is unchanged.

Current-context reads accept a JSON-encoded ordered `fallback` tail (empty disables
fallback; omission uses configuration). It participates in the context fingerprint.
Frozen plans/versions reject overrides. `/branches?datasets=<JSON UUID array>`
filters branches by local dataset heads, without reading data or providers.
