# Canonical JSON and content hashes v1

T015 fixes the initial byte encoding used by content hashes. This profile is an
application contract, not a claim to implement RFC 8785/JCS. The implementation is
in Rust `tf_protocol::canonical`, Python `transflow_worker.canonical`, and the web
`canonical.ts` helper. The latter hashes explicit supplied content; the UI does not
select competing computation or catalogue semantics.

## Exact encoding

Input is a decoded JSON value. Reject duplicate keys, invalid UTF-8 and malformed
JSON at the raw decoding boundary (the worker codec does this). Canonicalization
cannot recover duplicate keys or precision already discarded by another decoder.

- Object keys sort by Unicode scalar value order (equivalently UTF-8 byte order
  for valid strings), including supplementary characters. JavaScript's default
  UTF-16 sort is insufficient. Object insertion order is irrelevant.
- Arrays retain order. This applies to files, schema fields, inputs and nested
  values; callers must order set-like content deterministically before hashing.
- Strings retain every Unicode scalar without normalization. Composed and
  decomposed accents differ. Unpaired surrogates are rejected. Non-ASCII characters
  are literal UTF-8. Quote/backslash are escaped, `/` is not. The five standard
  short control escapes are `\b`, `\t`, `\n`, `\f`, `\r`; remaining U+0000–U+001F
  use lowercase four-digit `\u00xx` escapes. U+2028/U+2029 are literal UTF-8.
- Null/booleans use their lowercase JSON spellings. Bare numbers must be finite,
  integral and between -9007199254740991 and 9007199254740991. Encode them as decimal
  integers with no leading zero or plus. Integral metadata such as `3.0` becomes
  `3`; bare negative zero becomes `0`. Fractional/unsafe bare numbers fail.
- Lossless wire strings remain exact: u64 maxima, decimal coefficients/scale,
  timestamps, float special values and signed zero never pass through binary64.
  Numerically equivalent string spellings may have different fingerprints. This
  is conservative representational identity, not SQL or logical table equality.
- No whitespace, byte-order mark or trailing newline is added. Root depth is zero;
  depth above 64 and canonical output above 16 MiB fail. Limits cover metadata;
  file content hashing streams separately in 64 KiB chunks.

## Hash domains

All canonical content hashes are SHA-256 of **ASCII prefix, one NUL byte, canonical
JSON bytes**, in that order. Prefixes are fixed and versioned:

| Purpose | Prefix | Selection |
|---|---|---|
| Physical artifact | `transflow.artifact.v1` | All validated `ArtifactManifestV1` fields |
| Source capture | `transflow.source.v1` | Explicit source-content descriptor |
| Semantic computation | `transflow.compute.v1` | Explicit selected computation semantics |
| Catalogue references | `transflow.catalog.v1` | Snapshot format version, workspace ID and ordered entries |
| Logical schema | `transflow.schema.v1` | All validated `LogicalSchemaV1` fields |

A raw file digest is ordinary **unprefixed SHA-256 of every file byte**, used in
manifest file records. It cannot be requested through the canonical JSON hash API.
The Rust/Python digest objects retain their purpose as well as lowercase hex bytes.
No digest function allocates or derives a publication UUID.

`artifact_digest` checks the closed manifest shape and recomputes its schema
fingerprint before hashing. T037 will verify paths/lengths and actual file bytes;
this helper cannot establish that caller-supplied file assertions are true.
T012 shape-only fixtures with placeholder schema fingerprints are not sealed objects.

`catalog_fingerprint` validates the snapshot then selects `format_version`,
`workspace_id` and `entries`. The self-referential fingerprint and independent
`source_snapshot_id` are excluded. A changed dataset owner/ID/path/kind or entry
order changes the fingerprint. `verify_catalog_fingerprint` rejects stale/tampered
values. Source snapshot identity is still separately bound by the caller.

`semantic_fingerprint` accepts exactly `{semantic, presentation}` for source or
compute content and hashes only `semantic`. The explicit presentation boundary
excludes descriptions, graph positions and colours supplied there. It does **not**
recursively remove names such as `description` from parameters, configuration or
ASTs. Check policies/ASTs and exact input identities belong in semantic content.
T023/T050 must select/validate complete source and computation descriptors under
architecture 11.1; this primitive is not a cache key builder for arbitrary producers.
Captured source bytes may change after a docstring edit, causing a conservative
rebuild even when an independent presentation label would not change the key.

## Verification and evolution

[Fixed golden vectors](fixtures/canonical-v1.json) contain 21 encodings, 105
purpose-separated digests, five rejected numeric cases, physical/catalogue/semantic
examples and raw SHA-256 vectors. Rust, Python and TypeScript test the same constants.
They were fixed using a separate stdlib JSON/hashlib reference, not generated from
one of the production implementations during tests. Language-specific tests cover
byte/depth limits, invalid values, I/O errors, manifest integrity, display exclusion,
input-version changes and independent publication identities. Raw SHA-256 also
checks the standard empty/`abc`/million-`a` answers.

Changing byte rules or field selection for a purpose requires a new hash prefix
version and compatibility/migration decision. This initial profile activates no
cache, object store, source capture or publication path. Those remain later tasks.
