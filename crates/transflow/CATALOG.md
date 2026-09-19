# Catalogue inspection and explicit lifecycle changes

```bash
transflow catalog list --limit 50
transflow catalog list --limit 50 --cursor '<previous next_cursor>'
transflow catalog show raw/orders
transflow catalog show dataset:947e0218-2527-4168-babd-448fbc469d93
transflow catalog rename raw/orders curated/orders --keep-alias
transflow catalog rename raw/orders curated/orders --keep-alias --yes
transflow catalog remove curated/orders
transflow catalog remove curated/orders --yes
```

Root `--workspace` and global `--json` work with these commands. Rename/removal
also accept `--python` with the same base-interpreter meaning as validate/sync.
List/show require no Python environment and never import source. Cursor pages
contain at most 100 exact local/external identities, sorted by path, and include
tombstones. Entries show up to eight aliases with exact alias counts. A cursor from a different semantic registry revision is rejected.
Show accepts exact canonical paths, explicit aliases and local dataset UUIDs;
tombstones remain inspectable but cannot resolve as active pipeline inputs.

Local entries include retained version counts and up to ten recent version IDs
with schema availability. Provider versions are explicitly uninspected. Branch
context is the configured workspace default; selecting/resolving build branches
remains later work. Producer locations come from digest-verified retained graph
and source snapshots and are always labelled potentially stale. These are display
facts, never authorization to execute old definitions after failed validation.
Read-only SQLite queries can create WAL/shared-memory coordination sidecars; they
never create a missing runtime database, migrate it or change domain rows.

Rename/removal first capture and structurally validate the current complete graph.
Without `--yes`, the command returns an impact preview and leaves authoring, graph,
editor and domain state unchanged. Applying repeats validation and guards; it does
not blindly accept an old preview. Registered identities and historical versions
survive rename. `--keep-alias` retains the old path as an explicit alias. Without
it, old string paths and `C.old.path` expressions may break and must be edited by
the author. Python files are never rewritten automatically.

Removal refuses a current producer or Input referencing the identity, even with
`--yes`. Remove/update those declarations first. Runtime checks conservatively
block lifecycle edits while any build is active, any unexpired read lease exists,
or saved schedules/views exist. Precise template/view reference analysis belongs
to their later repositories; an unknown future active use is not assumed safe.
Removal moves the local identity/path/kind into registry tombstones and removes
aliases targeting it. No dataset version, artifact, source capture or history row
is deleted. External-registration removal is a separate later command.

Mutations use the same expected-old/new registry journal and runtime ownership as
sync. Lifecycle intents contain no allocated IDs; recovery verifies the exact
replacement digest and complete retained identity set. A failure after registry
replacement retains that durable change and may leave editor refresh incomplete;
retry/recovery does not allocate replacement identities. The previous graph stays
retained for stale display. A subsequent valid sync records the new graph.

Human and `CatalogResultV1` JSON results include preview/applied state, blockers,
impact counts and up to 64 source references. Full structural validation is not
limited to that display cap. Current runtime indexing retains stable IDs; the
current registry is the authority for names and tombstones, while historical
snapshots continue to describe their own catalogue revision.
