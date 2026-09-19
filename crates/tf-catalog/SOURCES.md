# Source enumeration and module indexing (T022)

`SourceIndex::enumerate(&Workspace)` walks only configured roots and returns a
sorted immutable allowlist of relative paths and observed sizes. Helpers, SQL and
other resources are included without reading or executing their contents.
`require_file` fails when an explicitly named computation resource is absent or
excluded. This index is metadata, not an immutable source capture (T023).

Hard exclusions include `.git`, `.transflow`, virtual environments (including
`pyvenv.cfg` detection), Python caches, local locator files and known credential
locations/extensions. Nested workspaces are excluded. User `source_exclude` globs
match workspace-relative paths; patterns without `/` match basenames at any depth.
`*`, `?`, character classes/ranges/negation and complete `**` path segments are
supported. A matching directory excludes its subtree. Invalid patterns fail;
brace expansion and backslash escapes are not supported.

Symlinks may refer to regular files inside configured roots, but cannot escape the
workspace, reach protected/excluded files, or cross nested workspace/environment
boundaries. Directory aliases/cycles fail rather than importing a tree twice.
Enumeration is bounded to 100,000 entries and 64 levels and rejects special files.
The capture service must re-enumerate and guard copied bytes against later edits.
Known filename exclusions cannot identify arbitrary secrets embedded in code;
explicit exclusions and the authoring contract still apply.

`transflow_worker.source_index.ModuleIndex.build(roots, captured_paths)` validates
names before discovery imports, under the selected Python interpreter. It performs
no filesystem reads or imports of user modules. It rejects duplicate modules,
module/package conflicts, namespace contributions across roots, and SDK/stdlib/
required-runtime shadowing. Runtime profiles can add protected modules, not remove
the SDK/engine defaults. Names are checked with Python's actual identifier, keyword
and Unicode-normalization rules. Non-importable resource paths stay in the capture
but are not discovered as modules. Immutable entries distinguish ordinary modules,
packages and namespace packages; helper-only modules are valid.

The discovery worker (T027) will connect the frozen allowlist and module index to
actual producer imports. Enumeration and indexing do not discover decorators or
claim that a graph is structurally valid.
