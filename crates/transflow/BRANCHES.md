# Output branch preparation (T041)

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

This is the output-branch foundation, not a public build or branch-lifecycle CLI.
The later build preparer still owns requested-target/plan acceptance and actual
capture/environment verification before using this service. Git changes never
rename/delete data branches. Audited explicit branch lifecycle and input fallback
remain T044 and T042; restoring a tombstone needs that separate explicit decision.
