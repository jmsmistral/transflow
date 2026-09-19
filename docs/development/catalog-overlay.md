# Catalogue binding and typing spike

T010 now has an executable SDK binding and mypy overlay prototype. Python 3.13.0
and 3.14.7 pass all 48 package tests on macOS arm64 with mypy 2.3.1. Twenty tests
were added for this spike. Interactive editor completion is still unverified;
T010 remains open for that acceptance item. The
[decision and remaining qualification](../../../transflow-spec/docs/adr/ADR-017.md)
record the boundary between these results and editor/product support. The
[local evidence snapshot](evidence/t010-macos-arm64.json) records counts and wheel hashes.

## What is implemented

`from transflow.catalog import C` imports a normal module in the single
`transflow` wheel. Importing it performs no workspace reads, engine imports,
network access or worker startup. Catalogue access needs an explicit
`transflow.testing.catalog_context` containing an immutable prototype snapshot
and matching expected fingerprint. No current-directory lookup occurs.

The internal `_catalog_prototype` module contains synthetic path/UUID snapshots
and a reference resolver. These are **not CatalogSnapshotV1 or the production
DatasetRef API**. Nodes capture workspace identity, dataset identity, path and
snapshot fingerprint; they never query a database or branch head. A node may be
both a dataset and a namespace. Namespace-only references and unknown paths fail.
Public names such as `path`, `fingerprint` and `dataset_id` remain usable as children
because node metadata is private. Nested test contexts restore on failure;
concurrent async contexts retain their own snapshots.

`python/tools/catalog_overlay.py` is a contributor library, not an installed
command. Given an explicit workspace and snapshot, `write_overlay` creates:

```text
.transflow/runtime/generated/<fingerprint>/type-stubs/
  manifest.json
  transflow/__init__.pyi
  transflow/catalog.pyi
```

The root stub explicitly forwards every existing SDK root export. Generated
node types have concrete read-only properties and no dynamic `__getattr__` escape
hatch, so misspellings are errors. The separate runtime node implements dynamic
lookup. The manifest and exact generated content must match the expected snapshot;
an existing stale or tampered generation is refused. New generations are staged
and renamed without replacing previous generations. A failed rename leaves the
previous generation valid and cleans staging. Symlink output paths are rejected.

This is a **mypy source overlay**, not an installed `transflow-stubs` distribution.
No generated `.py` file exists. The overlay is never added to runtime `sys.path`
by the generator. Even the deliberate negative control that inserts this
`.pyi`-only directory still imports the installed runtime SDK correctly.

## Reproduce the automated proof

Use the prepared, hash-locked contributor environments described in
[Python setup](../../python/README.md):

```bash
bash tools/check-python.sh
PYTHON_CHECK="$PWD/target/python/py313/bin/python" bash tools/check-python.sh
```

The packaging test builds and installs the actual wheel offline outside both
checkouts. Two concurrent mypy processes share that install but use separate
workspace configs, overlays and caches. One workspace has `C.raw.orders` and
`C.raw.orders.daily`; the other has `C.raw.invoices` and
`C.raw.invoices.daily`. Each accepts its own names and rejects the other's.
Tests inspect mypy's resolved member tables as well as positive/negative
consumer diagnostics. All SDK root exports, worker metadata and other SDK
submodules still resolve. Every installed file is hashed before/after and must
remain identical. No source checkout or ambient PYTHONPATH/MYPYPATH supplies
the consumer's runtime package or types.

The other fixtures cover empty catalogues, identifier/UUID errors, duplicate
paths/identities, canonical ordering, immutable objects, stale fingerprints,
namespace rejection, prefix datasets, two simultaneous runtime contexts, extra
runtime files, symlinks, tampering and interrupted generation. Root-stub exports
are compared against `transflow.__all__` to catch future SDK export drift.
Generated fixtures are temporary; they are not committed application contracts
in the T008 drift registry.

## Editor setup recipes awaiting qualification

Use a separate window/project for each workspace. The installed SDK interpreter
may be shared; the overlay path and cache must be workspace-specific. Configure
the selected mypy 2.3.1 process with an absolute workspace config:

```ini
[mypy]
strict = True
mypy_path = /absolute/workspace/.transflow/runtime/generated/FINGERPRINT/type-stubs
python_executable = /absolute/sdk-environment/bin/python
cache_dir = /absolute/workspace/.mypy_cache
```

For VS Code, a workspace task can run the already prepared tooling interpreter
with arguments `-m mypy --config-file /absolute/workspace/mypy.ini
/absolute/workspace/consumer.py`. Use a process task and a workspace-local working
directory. This is a diagnostics recipe; Pylance completion does not automatically
consume mypy's configuration. Do not add the stub directory to Python's runtime
search path. A separately qualified Pylance stub configuration or the later
Transflow language service is still needed for completion.

For Emacs, `M-x compile` can invoke that same explicit interpreter/config/consumer
command with the buffer's workspace as `default-directory`. This also establishes
only diagnostics. `lsp-mode` completion needs a qualified Python-server overlay
configuration or the later Transflow service. Do not assume two language servers
compose identically in VS Code and Emacs. Neither native completion UI was tested
in this session; VS Code is not installed on this host.

Remaining T010 qualification: in both documented editors, open the two synthetic
workspaces simultaneously, inspect `C.raw.` completion, reject a cross-workspace
typo, verify SDK root completions, switch to a new fingerprint, and confirm stale
metadata cannot be mistaken for the new context. Record editor, extension/server
versions and actual results. CLI/member-table evidence above is not a substitute
for observing those integrations.

## Product boundaries

The snapshot format, fixed entry limit and generator are feasibility scaffolding.
There is no worker lifetime binding, declaration normalization, foreign-output
policy, catalogue database, refresh pointer, durable fsync/recovery protocol,
source discovery or language service here. Directory rename proves local atomic
visibility for this test, not crash durability or protection against hostile
concurrent filesystem mutation. T012/T020/T028/T030/T031 and T107/T108 retain
those responsibilities. Application/specification versions and dependency locks
are unchanged; no additional packages, editors or browser drivers were installed.
