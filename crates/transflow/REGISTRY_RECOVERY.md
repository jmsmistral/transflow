# Registry reconciliation and recovery (T029)

The application service in `transflow::reconcile` joins the pure T028 proposal,
T023/T024 captured source, T019 runtime owner, guarded file operations in
`tf_catalog::registry_write`, and the `tf_store::catalog_mutations` repository.
No command dispatch, producer import, execution or dataset publication is added.
T033 composes catalogue sync; T049 composes build/draft acceptance after full
structural validation. Those callers must recover pending intents first and
surface conflicts before continuing.

A `ReconcileRequest` retains the exact proposed dataset IDs, source identity,
expected original byte digest and explicit working-tree/fixed-source context.
The mutation ID identifies the operation; it is not a dataset ID. No allocation
occurs in reconciliation or recovery. Empty additive proposals preserve the
registry's bytes and inode and do not create an intent.

For an additive working-tree proposal, the service:

1. Checks owner/workspace identity, capture identity, captured registry bytes,
   allowlisted source/config/dependency bytes and membership, and Git branch/commit.
2. Commits a PREPARED SQLite intent with old/new raw digests, every exact proposed
   path/ID assignment, source/workspace identity and complete local identity index.
3. Writes an exclusive sibling temporary file, preserves Unix permission bits and
   syncs it. It verifies temporary bytes and rechecks source/registry guards.
4. Atomically renames the file and syncs its parent, then verifies the new registry
   and unchanged source inputs. Descriptor and directory identities are checked.
5. Inserts exact local IDs and changes the journal to INDEXED in one SQLite
   transaction. Existing historical identities are retained. No version/head rows
   are written, so a registered dataset can remain unbuilt after later failure.

The runtime owner moves onto a dedicated blocking worker for this short operation
and is returned with either its result or failure. Dropping the waiting future
cannot release ownership while that worker is writing. Once dispatched, the
short durability section finishes; long producer execution and cancellation are
separate. File hashing/renaming does not block the caller's async executor, and
no filesystem observer runs inside a SQLite transaction.

Recovery uses the durable intent and current file, without changing authoring
bytes or allocating IDs:

| Observed registry | Recovery |
|---|---|
| Exact old digest | Leave the file intact; retain the intent as CONFLICT/NotApplied and allow fresh preparation. |
| Exact new digest and exact ID mapping | Sync file/parent, index those same persisted IDs, and mark INDEXED atomically. |
| Neither digest, wrong mapping, or changed workspace | Surface Conflict, retain its evidence, and require explicit fresh reconciliation. |
| No outstanding intent | Do nothing. Completed recovery is idempotent. |

Recovery of a new registry is valid even if source code changed after rename:
registered identities survive, while acceptance of that stale source has failed.
Removed producers/tombstones do not erase historical ID rows. Renames and removals
remain explicit later command work; the service does not infer them.

Fixed-source and Git-ref proposals with additions are rejected before writing.
Their selected registry must already contain the identities. Metadata-only owners
cannot reconcile. No Git checkout/reset/stash is performed. Missing/corrupt
journal evidence fails closed; a failed index transaction leaves all its inserts
rolled back and the same pending intent available for recovery.

The qualified local-filesystem guarantee is whole-file rename plus durable
recovery, not a transaction spanning SQLite and TOML. Ordinary editors do not
participate in Transflow's lock. Immediate before/after byte and source guards
detect observable edits; POSIX rename cannot provide an atomic compare-and-swap
against an editor writing between the last check and rename. Recovery never
tries to overwrite an unexpected authoring file. Ordinary errors clean only the
operation's own temporary; a killed process may leave an ignored-by-discovery
sibling temporary for later maintenance. This is not a power-loss/hardware test.

Twelve application tests include real SIGKILL at all seven journal/file/index
boundaries, request cancellation, Git switches with identical trees, explicit
refs, changed source/config/lock membership, symlinks, directory substitution,
corrupted temporary bytes and injected I/O failure. Two additional SQLite tests
cover durable round trips, single-pending-intent enforcement, malformed evidence
and rollback during the second dataset insertion. A digest parser regression
checks canonical lowercase spelling and distinct hash roles.
