# API contract boundary

Rust owns the domain and API schemas. T074 generated TypeScript contracts live in
`../generated/contracts.ts`; do not duplicate backend shapes here.

T077 adds `EventCursor`, a workspace-scoped sequence/UUID deduplicator for resumed
SSE facts. Events invalidate contextual reads; they are not a second job state
machine. An explicit resync refetches read models before accepting a new baseline.
The connected shell and stream lifecycle remain T082; the current React preview
does not connect to a coordinator. Graph interactions and ELK layout remain T083.

Authentication, browser fetch streaming and replay rules are described in the
[HTTP guide](../../../crates/tf-api/README.md).
