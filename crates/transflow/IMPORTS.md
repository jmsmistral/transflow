# Local Parquet import preparation

T035–T036 provide explicit copy staging and logical normalization before later
checks and publication:

```bash
transflow --workspace /path/to/workspace dataset import raw/orders \
  --path '/path/to/snapshot/*.parquet' --prepare-only --python /path/to/tooling/python
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

Success returns `status: prepared`, `published: false`, counts and a staging path.
Its closed `prepared.json` records the normalized logical schema and fingerprint,
source/copy paths, hashes, source capture, catalogue/environment and structural validation fingerprints. `--branch` records
branch intent (default: configured workspace default); it creates no data branch
or head. Later publication must resolve and validate its own accepted context.
The copies remain private preparation files, not sealed artifacts or versions.

Omitting `--prepare-only` fails clearly before workspace mutation. Logical
normalization is complete. Data-quality checks, durable artifact installation and
publication remain T037–T038/T064. Preparation cannot be consumed as published data.
Interrupted staging may remain for later runtime cleanup; ordinary failure
removes only files created by this operation, preserving unexpected files.

T036 updates preparation results to `schema_normalization: complete`. Earlier
preparation manifests lacking the logical schema/fingerprint must be prepared
again; they were never published versions.
