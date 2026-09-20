# Output branch preparation and lifecycle (T041, T044)

`tf-catalog::git::select_output_branch` selects a name once, with its origin:
explicit API/CLI override, recorded request/schedule choice, attached or unborn
symbolic Git branch, then the configured default when Git is absent or following
is disabled. Detached automatic following and explicit Git refs without an
explicit/recorded output choice fail. Existing Git inspection reports broken or
inaccessible metadata as errors; callers must not turn those into Git absence.
A recorded request need not inspect the operator's current checkout.

`Reader::preview_output_branch` reads existence/revision and never creates a row.
Before runtime initialization, `OutputBranchPreview::without_runtime` represents
absence without creating a database. A plan can display “branch will be created”.
Tombstoned names fail explicitly, including when the deletion happened after a
preview. There is no automatic restore or implicit copying of another branch's
heads. Names retain exact Unicode/case/slash spelling and remain metadata.

The composition service separates blocking structural authorization from its
short asynchronous repository mutation. `authorize_output_branch` requires a
successful `ValidatedGraph` matching the exact validation context and completed
registry reconciliation. It binds the selected workspace/name to source and
certificate evidence. `ensure_output_branch` then requires the actual writable
runtime owner and rechecks the preview's existence/revision in the transaction.
A new empty branch and its audit record commit atomically. An existing matching
branch returns the same ID without allocating another branch or audit row.

The later build preparer still owns requested-target/plan acceptance and actual
capture/environment verification before using this service.

## Explicit lifecycle commands

The public CLI supports `branch list`, `branch create <name>`,
`branch rename <old> <new>` and `branch delete <name>`. Create/rename/delete accept
`--dry-run`. Delete previews until `--yes` is supplied. List accepts `--limit`
(1–100, default 50) and an ID-keyset `--cursor`; tombstones are shown explicitly.
All commands support the normal human and closed CLI JSON envelopes. Branch names
remain exact metadata in JSON (bounded at 4,096 UTF-8 bytes by the service); terminal
output escapes visual-order controls and renders fallback names individually.

Read-only list/preview never creates runtime storage. Manual create adds an empty
branch; existing live names are idempotent and deleted names cannot be recreated.
Rename preserves the stable branch ID, historical heads and provenance. Delete
marks a tombstone and preserves retained versions/head pointers; it does not
remove artifacts. Mutation and audit commit together, with identity/revision and
active references rechecked under runtime ownership in the SQLite transaction.

Rename/delete report and refuse authored default/fallback references, saved view
and schedule references, active builds/reservations and live read leases. Update
those references deliberately before retrying. The initial saved-context check
conservatively matches exact branch ID/name scalar values in retained JSON;
unrelated same-valued metadata may therefore block a change. Future typed
view/schedule editors can refine those diagnostics. Both the source and destination
names are checked against authored policy for rename. No command edits
`workspace.toml`, Git branches, working files or another data branch's heads.

Git rename/delete/merge never renames, deletes or merges data branches. A running
persistent coordinator's owner lock prevents the temporary CLI owner from writing;
RPC dispatch into the persistent service remains later work.

`Store::snapshot_branch_policies` stores source-indexed immutable authored rules
in `branch_policies`, including meaningful empty rules. The empty starting-branch
sentinel stores the authored default tail (an empty user branch name is invalid).
`Reader::branch_policy_snapshot` reconstructs that captured context. Identical
writes are idempotent and changed snapshots fail. Historical policy rows do not
become mutable runtime defaults or prevent later lifecycle changes. The build
preparer will call this service alongside its source capture; accepted requests
continue using their recorded policy rather than re-reading edited configuration.
