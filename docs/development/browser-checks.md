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

## Branch picker and theme controls

Open the branch picker. Moving the pointer outside it must keep it open, as must
clicking its search field. Clicking a different widget or the canvas must close it and
still activate the clicked control. Escape closes it and restores branch-button
focus. Verify both light and dark themes: unchecked build-strategy radios must use
the selected app theme, and selected code text must retain legible foreground and
background colours across syntax tokens. The system colour preference must not
override the explicit app theme.

## History summary presentation

Select a dataset with measured jobs and open History. Row dates and times must
have visible spacing and omit seconds; row durations use two decimals. Summary
sits on the left of the date-filter row, with From/To/Apply grouped on the right
and wrapping within a narrow panel. Mean comes before Median; both metrics,
the chart axis and mean caption use two decimal places. Hover a duration point:
its tooltip contains only status, date/time to the minute and rounded seconds.
It must omit raw nanoseconds and attempt identifiers. Applying a same-day range
and navigating from a job detail back with Show summary retain their behavior.

## Build report controls and cached timing

Open a Build report with earlier comparable successful builds. Estimated must show
an approximate historical total duration and expose its cohort/sample explanation.
The status filter, path input and chart-mode buttons share a height and centre
vertically; the status filter has an accessible name without a visible label.
Open a cached-only build: Started and Ended display the same completion time,
Queue wait shows a not-applicable dash, and no synthetic attempt/bar or execution
duration appears. A first build without historical samples retains an unavailable
estimate; noncached missing starts/waits remain unavailable. Status/path filtering
and active-build refresh still work.

Check that manual single/multiple-target headings show the dataset path/count,
and scheduled headings show the schedule name. Open the custom status picker:
search, move with arrow keys and choose with Enter; selection returns focus to
the trigger. Escape closes just the picker, leaving the Build modal open. Clicking
outside closes it while allowing the clicked widget to act; moving outside does
not close it. Clear the filter to restore all jobs without shifting the time axis.
Names share a content-sized column capped at 240 pixels, with full-name tooltips
and ellipses for longer paths. The first track starts 12 pixels after that column;
the time axis aligns with the tracks. Bars have square corners and no glyphs.
Hover recorded/open bars to check duration units and the explicit elapsed label.

## Persistent review workspaces

For owner review sessions, create feature-specific synthetic workspaces under the
sibling `../transflow-workspace/` directory, such as `t095-schedules/`. Keep setup
and authenticated launch helpers there too, so a restart does not remove them.
Reserve temporary directories for automated fixtures that clean up after themselves.
Preserve existing review workspaces; remove them only when the owner requests
cleanup. Stop any coordinator started for verification before handing the workspace
back, and report its path and the observed checks.

For a workspace with an active schedule, leave the coordinator observing while
opening and filtering Catalogue, adding a dataset to the graph and inspecting its
preview. Routine clock/event cursor audits must not cause context-conflict errors.
Confirm separately that an actual publication still refreshes the displayed head.

## Saved lineage naming (T084)

Open Save for a new lineage and Save as for an existing one. Each must show an
empty full-width field with “Enter lineage view name…” and no visible name label
or prefilled copy name. The field retains its accessible name. Click or Tab into
it and confirm the hint disappears; blur an empty field and confirm it returns.
Empty and whitespace-only names keep Save lineage disabled. Enter a name and
verify keyboard saving and a new saved identity for Save as. Cancelling preserves
the current view and returns focus to the initiating control.

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
