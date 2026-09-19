# Optional Git source provider (T024)

The Git adapter inspects an explicit workspace and its ancestor Git marker. With
no marker it returns `None` without invoking Git. Invalid/inaccessible metadata
and missing Git are errors, not a silent Git-free fallback. It records the owning
repository and workspace-relative root, attached/unborn/detached state, exact
commit and pre-command dirty captured paths. A separate optional registry-overlay
record is reserved for reconciliation; these operations never edit the registry.

`output_branch` applies explicit branch, attached symbolic branch when enabled,
and configured default in that order. Detached following requires an explicit
branch; every explicit ref requires one, even when it names the current branch.
No database branch is created by inspection or capture.

`capture_working_tree` adds optional Git provenance to the filesystem capture.
`capture_ref` resolves its selector once to a commit, reads the selected workspace
configuration and only the selected authoring/source paths, and copies raw blobs
into a private managed directory. The original registry workspace ID must match.
Source exclusions apply before blob reads. Nested workspaces/environments remain
excluded; unsupported submodules and unsafe symlinks fail. The ordinary guarded
capture then installs retained bytes into the requesting workspace's runtime.

This provider uses a managed capture rather than checking out a worktree. It
never runs checkout/reset/stash, hooks, smudge filters or archive substitutions.
Git's optional index writes and lazy network fetching are disabled; commands have
30-second deadlines and bounded metadata output, while blob bytes stream to disk.
The source fingerprint remains about copied content, not the display ref name.

Real Git tests include a nested workspace, unborn and detached HEAD, dirty source,
foreign workspace identity, broken metadata, and a ref with export attributes.
They assert unchanged user HEAD/status/files after ref capture. Complete build/
plan CLI dispatch and registry-overlay application remain later tasks.

Plumbing formats are based on the official [ls-tree](https://git-scm.com/docs/git-ls-tree),
[cat-file](https://git-scm.com/docs/git-cat-file),
[symbolic-ref](https://git-scm.com/docs/git-symbolic-ref) and
[rev-parse](https://git-scm.com/docs/git-rev-parse) contracts.
