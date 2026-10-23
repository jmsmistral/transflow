# Contributing to Transflow

The [root README](../../README.md) introduces the product. The
[documentation index](../README.md) covers using it; this guide covers working on
its implementation.

## Orient in the repositories

The implementation has three main boundaries:

- [Rust crates](../../crates/README.md): coordinator, CLI, metadata, planning,
  publication and supervision.
- [Python SDK and worker](../../python/README.md): authoring declarations,
  captured execution, Polars and DuckDB integration.
- [Web workspace](../../web/README.md): connected lineage, previews and inspectors.

The authoritative contributor specification is the separate sibling
`transflow-spec` repository. Start implementation work with
[AGENTS.md](../../AGENTS.md), then its task-scoped navigation and engineering
agreement. Installed applications and user workspaces do not need the spec repo.
Task identifiers, dependency graphs and acceptance progress belong in that ledger;
actual measurements belong in [verification](verification.md) and its receipts.

## Set up and verify

The [source quickstart](../getting-started.md) builds a working local preview.
For the prepared Operational alpha review sample, use the
[CLI checks guide](sample-workspace-cli.md), including PATH setup and commands
without `--workspace`.
For development, follow the [verification workflow](verification.md#verification-workflow),
[compatibility guide](compatibility.md) and [safety policy](safety.md). Toolchain and
runtime dependencies are pinned; dependency installation is an explicit setup step.
The normal checks operate offline against already-prepared environments.

Use focused checks while editing, then the required final gates for the affected
contracts. Complete documentation and formatting before that final run. Keep full
logs locally and record compact, reproducible results; distinguish local, CI and
manual editor/browser evidence.

Sequential owner development currently uses `master` directly in both repositories.
Rebuild the UI before handoff and rebuild the CLI when its source changes. A review
workspace needs matching artifacts and a Bash launcher; preserve bundles while
someone is reviewing them. The [review launcher](../../tools/serve-review.sh)
builds the current checkout's CLI and UI before serving them.

## Keep the introduction readable

Preserve the root README's product-first structure: short introduction, real demo
visual, a small authoring example, benefits, quickstart and documentation links.
Keep it approximately 100–150 lines, with no milestone diary, task IDs, test-count
history or detailed architecture/operational contracts. Update a feature sentence
or guide link when behavior changes. Add reference detail to its canonical guide
and keep the [documentation index](../README.md) discoverable.

Use only original, synthetic demo data in public screenshots or recordings. The
current screenshot is a real verified UI capture; a future short terminal/UI GIF
can replace or accompany it after recording the actual working flow.

See [Python release readiness](python-release-readiness.md) before proposing a
public package upload. Publication is a separate owner-approved action.
