# Transflow documentation

Start with [Getting started](getting-started.md) for a working source installation,
a first dataset build and the lineage workspace.

## Write datasets

- [Python declarations](../python/DECLARATIONS.md) — inputs, outputs and transforms.
- [Polars execution](../python/POLARS.md) and [DuckDB SQL transforms](../python/SQL_TRANSFORMS.md).
- [Quality checks](../python/EXPECTATIONS.md) — exact checks and failure policies.
- [Catalogue references](../python/CATALOG.md) — stable dataset references in Python.
- [Customer-orders walkthrough](../examples/customer-orders/README.md),
  [SQL and checks example](../examples/sql-checks/README.md), and
  [operational demo](../examples/operational-alpha/README.md).

## Build and operate

- [Builds, plans, history, cancellation and replay](../crates/transflow/BUILDS.md).
- [Workspaces and configuration](../crates/tf-catalog/WORKSPACE.md).
- [Python environments](../crates/tf-exec/ENVIRONMENTS.md).
- [Catalogue inspection and lifecycle edits](../crates/transflow/CATALOG.md).
- [Branches](../crates/transflow/BRANCHES.md).
- [Schedules](../crates/transflow/SCHEDULES.md).
- [External workspaces and direct reads](../crates/transflow/EXTERNAL.md).

## Explore the workspace

- [Lineage UI reference](../web/README.md) — graph exploration, saved views,
  previews, SQL scratchpad, code, history and inspectors.
- [HTTP API](../crates/tf-api/README.md).
- [CLI conventions and diagnostics](../crates/transflow/README.md).

## Contribute and release

- [Contributor guide](development/README.md).
- [Verification workflow and measured evidence](development/verification.md).
- [Compatibility and toolchains](development/compatibility.md).
- [Safety and dependency policy](development/safety.md).
- [Python release readiness](development/python-release-readiness.md).

The root README is the project's introduction. Detailed contracts, implementation
milestones and verification history belong in these guides and the contributor
specification.
