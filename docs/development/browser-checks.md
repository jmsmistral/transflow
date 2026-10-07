# Internal Browser verification

Use Codex's internal Browser only. Do not install Playwright, browser drivers or
browser binaries. Vitest/jsdom proves component behavior, not native browser focus,
layout or coordinator integration.

For T082, prepare a synthetic workspace with published versions on two branches,
build `web/dist`, and run the coordinator with the explicit `--ui-dir` option in
[web setup](../../web/README.md). Obtain its one-use launch grant without exposing
the bearer token.

1. Open the launch URL. Verify Connected, fragment removal and actual bounded
   catalogue metadata. Read browser errors; no fake successful build is displayed.
2. Select a dataset by keyboard. Inspect requested/resolved branch, definition
   source and head. Select an exact version; verify historical context and producing
   source, with current-head metadata explicitly unavailable in that context.
3. Switch branches and selections. Verify labels and metadata match, old content
   clears while loading, and panel sizes/active tab survive. Deterministic component
   tests separately inject delayed, reordered and abort-ignoring responses.
4. Resize by keyboard, use arrow/Home/End tab navigation, collapse/reopen panels and
   toggle themes. Verify visible focus and names. Narrow the viewport to about
   390 × 844; check stacking/scrolling and no horizontal overflow. Restore it.
5. Build the synthetic dataset through the CLI. Verify committed events refresh
   its head without manually reloading or guessing job state.
6. Stop the coordinator. Verify explicit disconnected guidance and removal of old
   dataset content. Reload without a launch grant and confirm no authenticated
   data appears. Use a fresh grant to reconnect.
7. Record exact build hashes and observed checks separately from automated suites.
   Do not claim a full screen-reader audit, complete graph/inspector behavior or
   end-to-end preview/source rendering before those tasks ship.

Keep private reference screenshots and credentials out of committed evidence.

## Persistent review workspaces

For owner review sessions, create feature-specific synthetic workspaces under the
sibling `../transflow-workspace/` directory, such as `t095-schedules/`. Keep setup
and authenticated launch helpers there too, so a restart does not remove them.
Reserve temporary directories for automated fixtures that clean up after themselves.
Preserve existing review workspaces; remove them only when the owner requests
cleanup. Stop any coordinator started for verification before handing the workspace
back, and report its path and the observed checks.

## Dataset inspection (T085)

On a real synthetic coordinator, select a published local dataset and open the
right Properties and bottom Preview panels. Check creation/publication times,
stable identity, producer, schema, exact physical counts and returned rows. The
Preview has one compact dataset row above the table; a blue warning names the
resolved branch only when different from the selected branch. Click and drag cells,
select the displayed rectangle from the top-left corner, and copy via the
right-click menu or Cmd+C/Ctrl+C with headers. Right-click outside a selected
range and confirm Copy still uses that range; the menu has no sensitive-data
label. Check that headers preserve schema field-name case,
a short table leaves an unruled blank area after its fixed-width columns even
when all displayed cells are selected, and no more than 1,000 rows can be
displayed. Exercise an absent selected-branch head: ordered fallbacks precede
the browse-only retained-branch suggestion. Page requests
must keep the initially bound exact version. Select a registered foreign dataset:
metadata counts come from its retained manifest, while provider byte availability
and freshness remain distinct; a preview reads the provider under its lease.
Exercise unavailable schema/read. With at least two visible nodes, select both,
open Properties and Preview, and confirm each shows “Select a node to view
information” centered in each content area with an info icon. Check the other
bottom tabs, then
plain-click an already selected node and confirm it alone is selected and its
single-node inspectors return. Click empty canvas and confirm the selection clears,
Preview's table disappears and both inspectors show the same centered notice.
Selecting a node again restores its inspectors. Check modifier-click multi-selection.
Record actual browser observations separately from deterministic component tests; no private
reference images enter the repository.

## Graph journeys (T083)

Use a synthetic diamond, validation-only foreign boundary and a fan-out above 500
nodes. Confirm depth 0/1/unlimited counts, endpoint-complete edges, deduplication,
path queries with hidden intermediates, list checkboxes, keyboard position pinning
across expansion, explicit relayout, whole-catalogue search beyond its first page,
and branch reset. Step through batches to the 500-node guard, opt in and exhaust
all remaining pages. Check worker errors/CSP, viewport fit/zoom, narrow screens and
unchanged domain heads. Distinguish actual Browser observations from unit tests.
