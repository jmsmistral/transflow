# Snapshot-bound catalogue references (T030)

`from transflow.catalog import C` uses a validated, immutable `CatalogSnapshotV1`.
A fresh discovery worker binds its snapshot once before importing user modules.
The same binding is visible to its Python threads and cannot be replaced through
the testing API. Lookup never searches the current directory, imports producers,
opens SQLite or selects a moving dataset version.

`Input(C.raw.orders)` and the corresponding exact string resolve to the same
owner-qualified dataset identity. Nodes can be both datasets and namespace
prefixes: `C.raw.orders` and `C.raw.orders.daily` may coexist. Namespace-only nodes
cannot be passed to Input/Output. Metadata uses private members so names such as
`path`, `fingerprint` or `dataset_id` remain usable dataset segments.

Explicit local rename aliases and alternate foreign registration paths are
included in the snapshot. An alias retains its own path plus the target's stable
owner/ID; foreign branch policies still belong to the selected registration.
`C.external` is reserved for foreign read boundaries. External outputs and unknown
C attributes fail; a typo does not create a dataset. Declare new outputs with
strings until catalogue/build command integration is available.

The catalogue fingerprint includes ordered canonical entries and optional ordered
alias mappings. It excludes the self-fingerprint and source snapshot ID. Snapshots
without the optional alias field retain their previous digest. Aliases may only
target an included canonical entry, and every name is unique and ownership-valid.
Older closed-schema readers reject alias-bearing snapshots instead of silently
omitting names. The unpublished, matched SDK/worker distribution must be used.

For direct Python tests, supply a coordinator-produced or synthetic wire document:

```python
from transflow.catalog import CatalogSnapshot
from transflow.testing import catalog_context

# captured_catalog_document is a decoded CatalogSnapshotV1 with a verified digest.
snapshot = CatalogSnapshot(captured_catalog_document)
with catalog_context(snapshot, expected_fingerprint=snapshot.fingerprint):
    # Import your own catalogue-dependent module inside this explicit context.
    import my_transforms.orders
```

A snapshot makes a detached private copy of the document. `snapshot.document()`
returns another detached copy for transport/test inspection. Tests can explicitly
nest contexts or run independent async contexts; exit restores the previous
binding even after exceptions. This does not clear Python's module cache. Import
or explicitly reload your own modules in the intended context; retained nodes
continue to refer to their original snapshot and fail a mismatched fingerprint.
No implicit context is inferred from the working directory.

`resolve_reference(node, expected_fingerprint=...)` exposes immutable `DatasetRef`
identity for inspection. A node's captured identity remains stable after its test
context exits. A worker owns one process-wide immutable binding for its lifetime;
test contexts cannot override it, and a second worker bind is an error.

Snapshot indexing is bounded to 16 MiB of JSON, 100,000 names, 4,096 characters per
path and 64 path segments. These are metadata-service limits, independent of
transform memory policy. The private `_catalog_prototype` adapter only retains
historical T010 fixtures and delegates runtime behaviour to this implementation;
it is not a second binding protocol or a public authoring API.

Runtime/context tests include simultaneous installed subprocess workspaces,
worker-thread lookups, alias ownership and fingerprint tampering. Shared alias
contract cases and a fixed digest vector are checked by Rust, Python and
TypeScript. Editor overlay production refresh and native-editor qualification
belong to T031; these runtime tests do not claim native editor completion.
