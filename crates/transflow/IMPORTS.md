# Local Parquet imports

T073 connects explicit local-file staging and normalization to guarded publication:

```bash
transflow --workspace /path/to/workspace dataset import raw/orders \
  --path '/path/to/snapshot/*.parquet' --python /path/to/tooling/python
```

Prepare the managed environment with `env lock` and `env sync` first. Discovery
uses its matched installed SDK/worker and imports trusted captured Python code;
it does not call producer functions. Relative file selections are relative to
the caller's directory. Quote globs so Transflow freezes their expansion once.
An exact file, `*`, `?`, bracket classes and complete `**` segments are supported.
The canonical directory prefix before the first wildcard is the explicit
containment root; traversal beneath it never follows symlinks or special files.
Selection is sorted and limited to 10,000 files, 100,000 visited entries and 64
levels. An empty selection fails. A valid zero-row Parquet file succeeds.

Preparation copies bytes into private runtime staging, never hard-links the
source. It decodes all rows using the pinned Arrow/Parquet reader, requires
matching normalized logical schemas across files, and supports uncompressed, Snappy
and Zstandard Parquet. It bounds footer metadata to 16 MiB and decodes in batches;
this is not a general memory cap. The [shared normalization service](../tf-store/NORMALIZATION.md)
checks portable types and exact value ranges; incompatible or lossy types fail explicitly.

Before accepting staging, Transflow rechecks file identities, lengths, timestamps
and SHA-256 digests. Observable source changes fail preparation. Concurrent
in-place writers cannot provide an atomic multi-file snapshot: supply an atomic
producer snapshot for that guarantee. Both JSON result and retained manifest
explicitly report `source_snapshot_limitation: true`.

An absent local target becomes an `imported` catalogue identity only after the
complete overlaid graph validates. Inputs may already reference that new target
using a string or `C`; unrelated pending outputs are not registered by import.
An existing imported path, alias or `dataset:UUID` keeps its identity. Producer,
foreign and tombstoned identities cannot be replaced. Once registration commits,
a later preparation failure preserves that identity and reports failure.

Success returns `status: published`, `published: true`, file/row/byte counts,
version ID, artifact digest and branch-head generation. Without `--branch`, the
attached Git branch is used; no-Git/unborn workspaces use their configured default.
Detached HEAD requires an explicit branch. Branch creation is lazy and atomic with
publication. Imports do not run producer functions or fabricate transform attempts
or quality-check PASS records. Consumer expectations run when a transform uses the
imported version. Imported boundaries are never rebuilt by `full` or `--force`.

Files are installed through the same durable artifact service as transform outputs.
An owner-fenced SQLite transaction records immutable import/source provenance,
version, head, head-change event and outbox record. Provenance retains the original
frozen source root/file list alongside the copied hashes. It rejects a changed head/branch,
active writer reservation or garbage-collection claim. A pre-commit failure keeps
the previous version visible; an installed but unpublished object is an orphan,
never a head. Successful publication removes its private import staging. No files
are re-encoded: artifact writer metadata uses `engine: import`, `compression:
preserved` and `row_group_size: "0"` to record that no codec or row-group size was
requested. Earlier imported versions remain usable by exact pins/replay under the
normal retention policy even after the original source files have gone away.

`--prepare-only` retains the earlier explicit staging mode: `status: prepared`,
`published: false`, counts and a staging path. Its closed `prepared.json` records
normalized schema/fingerprint, source/copy paths, hashes and captured catalogue,
environment and structural-validation context. Its branch is intent only (the
configured default unless explicitly supplied); it creates no branch or head.
Private staging cannot be consumed by transforms. Publish by importing again;
this command deliberately rechecks the source selection rather than trusting old
staging. Interrupted staging may remain for later runtime cleanup. Ordinary failure
removes only files created by this operation, preserving unexpected files.

This copying behavior is specific to explicit local-file imports. Registered
external workspace datasets use leased reads of provider-owned immutable files;
they do not copy external inputs into the consumer object store.
