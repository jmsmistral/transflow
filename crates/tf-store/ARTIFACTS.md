# Artifact objects (T037)

`ArtifactStore` copies an explicit ordered list of Parquet files into private
staging, decodes every row with the shared normalizer, and creates the canonical
`ArtifactManifestV1`. Each entry has an ordinal relative filename, full SHA-256,
byte length and row count. Every file has the same normalized logical schema.
Empty schema-bearing files are valid; an empty file list is not.

The caller supplies writer provenance; the service validates its closed shape,
but does not infer a producer's version or historical row-group configuration.
The digest covers the entire canonical manifest, including file order and writer
configuration. It excludes runtime identity because those fields are absent.
A physical digest does not allocate or identify a dataset publication.

All path traversal is descriptor-relative and rejects symlinks, nonregular files,
hardlinks and changed workspace/runtime directory identities. Copies are separate
inodes; observed source changes abort preparation. Atomic producer snapshots are
still required for a stronger guarantee against concurrent external writers.

`Candidate::install` verifies the sealed bytes, flushes files/manifest/staging,
then uses rustix `renameat_with(NOREPLACE)` on the same filesystem. Files are 0400;
the directory becomes 0500 after rename (macOS requires a writable directory during
its move). Both rename parents are flushed before returning a private
`VerifiedArtifact`. Existing objects are fully verified before reuse; they are
never overwritten or repaired behind an existing digest. Errors after rename
leave an orphan object, without creating any version or head.

Strict verification reads a bounded canonical manifest (16 MiB), exact directory
membership (at most 10,000 files), full content hashes, decoded schemas and rows.
Missing bytes, changed footers/content, extra files and noncanonical metadata fail.
There is no metadata-only shortcut or persistent verification cache. File modes
are defense in depth against accidental writes, not a sandbox against the owner.

These blocking operations belong in a bounded blocking task holding runtime
ownership. The module does not elect a coordinator or publish a version. Read
leases/GC, execution adapters and the publication orchestration are separate
services. Durability assumes the qualified local filesystem/device honors flushes;
process interruption tests are not hardware power-loss qualification.
