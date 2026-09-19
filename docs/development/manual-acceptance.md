# Manual acceptance follow-up

The owner plans a comprehensive manual test after more implementation tasks land.
Record the implementation revision, OS/Python/editor/server versions, setup,
expected result and actual observation for each completed journey. These are
pending checks, not failures or claimed passes.

## Catalogue editor integration — T031/T108

T010's runtime/mypy prototype is complete for G0. Native editor completion remains
pending in both VS Code and Emacs. Use the synthetic contexts and setup constraints
in the [overlay report](catalog-overlay.md), once editor integration is delivered.

- Open two independent workspaces simultaneously: `raw/orders` in one and
  `raw/invoices` in the other. Each `C.raw.` completion must show its own names.
- Verify SDK root exports and ordinary submodules remain available in both.
- Confirm a cross-workspace reference and a misspelled C name produce diagnostics.
- Refresh one catalogue fingerprint; confirm its completion changes while the
  other workspace remains unchanged. Stale captured references must fail.
- Verify runtime imports still load the installed SDK, with no generated runtime
  package or installed-file changes.
- Record any limitations when combining the Python and Transflow language servers.

Add executable build, validation, publication, history and UI journeys here as
those features become available, linking each to its implementation task/scenario.
