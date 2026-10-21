# Transflow workspace and lineage

## Operational-alpha inspectors (T099)

SQL scratchpad queries the displayed immutable version as `dataset`. Run query
retains its exact bindings and query receipt; changing selection or
leaving the pane cancels owned active work. Results use the same typed read-only
grid and safe clipboard format as Preview. Truncation, failure and cancellation
states remain visible. Result expiry does not publish
data or substitute a moving head. Fallback data has Preview's branch warning.

All five bottom tabs share a full-width, 44-pixel dataset header with a 13-pixel
name. The name stays on the left; controls and metadata keep their context without
repeating the tab title. Long names truncate visually and retain their full tooltip.
The right Health inspector scrolls its details separately from this header.

The scratchpad uses an editor on the left and an always-present Results pane on
the right. Run query (play icon) and Cancel query sit beside the dataset name,
above the editor. The initial Results notice has a local database glyph and the
prompt “Run a SQL query in the editor to see a preview of the results.” Results,
errors and cancellation stay within that pane. Compact panes
use a smaller empty state; narrow layouts stack the two panes.

Enter literal values directly in SQL; the Parameters controls are removed. A small
rotating icon shows pending work beside Cancel query and stops on a
terminal state or unknown status. Reduced-motion preferences disable rotation.
Successful results show row count and duration without the SUCCEEDED prefix or
repetitive limits footer. Failures and truncation remain explicit.
Row count, duration and Download TSV share the dataset toolbar, aligned right;
Download TSV matches Run query's button height. The results pane has no heading row.
SQL parser, binder and conversion errors retain their useful engine explanation,
including line breaks, rendered as plain text. Failed queries do not show the initial
run-query prompt below the error.

Download TSV includes headers and every available result page, up to the service's
bounded result. It uses Preview's field quoting and spreadsheet-formula protection;
NULL and exact typed text are preserved. A failed, expired or inconsistent page
blocks the entire download. Leaving the selection cancels pending reads and
releases the owned download URL. Empty results download their column headers.

The default query execution budget is 30 seconds, configured by
`interactive.query_timeout_seconds` in `workspace.toml`. It covers setup and
result acceptance; queue wait and supervised termination grace are separate.
The API still supports bound parameters; the scratchpad submits an empty list.

Data health uses a pulse icon in the right inspector, headed by the dataset name without
a duplicate row or refresh control. Committed build events refresh it automatically.
Latest builds, Output dataset checks
and Input dataset checks each show ten newest-first status circles and a latest
status pill. Latest builds circles are status indicators. Filled output/input check
circles have a darker border and open their own retained checks
in an animated panel, with a status circle and text on each grey check card.
Click the selected circle again to close it; empty history slots are disabled.
Keyboard activation and reduced motion are supported. Missing history, absent checks and unavailable evidence stay neutral;
PASS is green, WARN violations amber and FAIL violations red. Build reuse is labelled
Cached and never invents a check run. Histories read up to 1,000 jobs through bounded
pages ordered by build creation time (`order=newest`) and disclose that limit when reached. Published checks use exact version
contexts; input checks use each attempt's retained plan. Provider certificates stay
retained metadata, without executing or contacting provider code.
Fallback output certificates use the displayed publication's branch. Latest builds
and input-check history remain on the selected branch, so fallback publication
does not invent a build there.

History centres summary metrics between its heading and date controls. Its duration
chart uses the available width and plots successful attempts only; failed jobs
remain in the list and failure-rate metrics. Publication availability no longer
adds an info badge to graph nodes. A failed latest attempt is labelled independently
from any retained earlier successful publication.

When the selected branch has no retained dataset jobs, History uses the displayed
data's resolved branch for its job list, summary metrics and job details. The blue
fallback banner and branch label name that branch. Any jobs on the selected branch,
including failures without a publication, take precedence. Explicit version pins
keep the selected History branch.

Code opens with Latest saved version first, Data publish version second and Diff
third. The first shows the latest validated capture. The published view always
uses the displayed data version's exact producer. Diff aligns the two captures in
side-by-side columns, with original line numbers, highlighted changes and shared
vertical scrolling. Missing or bounded source stays explicitly unavailable for diff.
All Code modes show the data fallback banner when applicable. Latest saved version
also explains that the current validated source capture is shared across data
branches; it is not a separate branch's working tree. Data publish version retains
the fallback output's exact source. Display fallback never changes build source.
Dynamic transforms place **Generated** before **Authored** beside the dataset
name and open in **Generated** by default. **Authored** remains selectable;
Diff opens authored source. Ordinary transforms continue to open in authored code.

Node hover cards separate Data quality from Freshness. Freshness compares successful
direct-input publications with the displayed output publication, following named
branches and fallback policy; newer inputs mean Out-of-date. Missing or foreign
input evidence stays Unavailable. This does not replace the broader Out of date
overlay, which includes code and inherited changes. All formatted physical sizes
use decimal KB, MB, GB, TB or PB; exact raw evidence remains available in details.

The [public demo](../examples/operational-alpha/README.md) uses real coordinator
state and synthetic data. Schedule-role changes retain measured node dimensions
as well as visual positions, preventing invisible nodes after controlled refreshes.

## Workspace foundation

T082 connects the original React interface to a local coordinator. It provides
bounded catalogue pages, requested branch/fallback metadata, dataset and exact
version selection, explicit retained/historical source context, connection states,
light/dark themes through a keyboard-operable sun/moon switch and resizable right/bottom panels. Tabs, splitters and selection
work by keyboard. Narrow screens stack panels; layout survives context changes
within the page session.

T083 adds the React Flow dataset graph, keyboard graph controls, typed data/validation
edges, foreign boundaries and stable manual positioning. An off-thread ELK layout foundation is retained for future explicit layout. T084 adds saved views (below). T085 adds properties, columns and exact-version preview (below). Code/history, build planning/reports and schedule actions are available. Selecting a version cannot run a build or change dataset heads.
Metadata availability is not byte verification or freshness evidence.

## Build and connect

Use Node 24.22.0/npm 11.19.1 and the exact lockfile. Installation is explicit:

```bash
npm --prefix web ci --ignore-scripts
npm --prefix web run prepare:types
bash tools/check-web.sh
bash tools/serve-review.sh /path/to/workspace --open
```

The review launcher rebuilds the local CLI and production assets before serving
them and fails without launching if either build fails. It does not install
dependencies or change the workspace's prepared Python environment. Pulling source
commits alone leaves `target/debug/transflow` and `web/dist` unchanged. Stop any
previous coordinator before launching: UI assets are snapshotted at startup.
Use the new authenticated page after restarting.

Workspace-specific `serve-preview.sh` helpers delegate to this launcher so each
run serves the current checkout. If a matching pinned Node toolchain has already
been unpacked under `target/toolchains`, the launcher uses it before ambient tools.

`--ui-dir` is an explicit contributor bundle directory. It serves a frozen snapshot
of `index.html` and flat `assets/*.js`/`*.css` files, up to 64 files/8 MiB total,
rejecting symlinks and unsupported entries. Rebuild and restart the coordinator
after changes. It never serves the workspace directory or arbitrary files.
Without that option the coordinator retains its small authentication bootstrap.
Native embedding and offline release packaging remain T117.

The browser exchanges a one-use launch fragment for a same-origin HttpOnly session
and in-memory CSRF value, then removes the fragment. Reload requires a fresh launch
grant; credentials are not stored in localStorage. If a coordinator is already
running, authenticated clients can request a new grant with the existing
`POST /api/v1/sessions/launch` API; see the [HTTP guide](../crates/tf-api/README.md).
Do not put the long-lived bearer token in a browser URL.

`npm --prefix web run dev` and `npm --prefix web run preview` still show the
unconnected shell. They do not proxy authentication to another origin. Node is
contributor tooling, not part of the eventual native runtime.

## Context and event boundaries

`src/api/client.ts` validates unknown responses against the authored schema and
uses the Origin/CSRF-protected read facade. `src/workspace.ts` owns one visible
selection epoch. It aborts superseded requests, compares the complete context and
rejects late replies even when a transport ignores cancellation. Inspector reads
use the same guard. No previous content is painted beneath a new selection label.
Conflicts restart pagination through refresh; disconnected/failed states carry no
old dataset snapshot.

`src/api/stream.ts` consumes bounded SSE facts and checkpoints via fetch POST.
Notifications trigger authoritative reads, coalesced per checkpoint. Replay gaps
are acknowledged only after successful refetch; a simultaneous user selection
wins. Network/session failures show explicit reconnect/relaunch guidance. Context,
source, graph and branch semantics remain backend-owned.

## Verification and scope

`tools/check-web.sh` verifies pinned tool versions, types, lint/accessibility rules,
formatting, shared contract/component/race tests and two byte-identical production
builds. It writes ignored results in `target/web/`. Browser journeys use Codex's
internal Browser; do not install Playwright, browser drivers or browser binaries.
See the [browser checklist](../docs/development/browser-checks.md) and
[T082 measured receipt](../docs/development/evidence/t082-macos-arm64.json).

Every jsdom file uses a shared text-table adapter for RevoGrid, including files
that only import inspector helpers. This keeps Stencil's import-time startup
timer inside browser qualification. Selection tests supply their own imperative
grid adapter. File isolation and unhandled-error detection remain enabled; real
grid layout, virtualization and gestures are covered by the internal Browser.

React Flow/ELK interaction and worker layout remain T083. The exact React 19.3.0,
TypeScript 6.0.3, Vite 8.3.0, jsdom 26.1.0 and ESLint 9.39.5 baseline is unchanged.
Existing development dependency exceptions remain in the safety inventory; no
new package was added. Public fixtures and browser journeys use synthetic data.

## Graph exploration (T083)

Select a catalogue dataset to add its root without reloading the workspace, moving existing nodes or changing the camera. Both inspectors start collapsed. Nodes use internal chevrons to expand/retract all immediate neighbours in bounded pages. Expanded neighbours are selected. External nodes stop at the provider boundary. New nodes occupy free positions; existing/manual positions never change on addition/removal.

The top-left toolbar contains **Layout** (placeholder), **Select → Clear all** (selection only), and **Expand** (forward/backward and levels from all selected nodes). Blank levels means all reachable nodes. Explicit expansion drains bounded pages and reconciles visible connections atomically. The bottom-right **N nodes selected** label centres the selection without changing zoom. Fit view/selection are separate zoom actions at the lower left. Messages appear at the bottom and expire after four seconds; the legend sits flush at the top-right. Controls use local SVG icons without a font or network dependency.

Dashed node borders indicate a confirmed missing local publication on the selected branch and fallbacks. External or historical publication metadata is unknown and stays solid, not falsely labelled never-built. All dependency edges are solid; validation uses an accent colour. The API publication state does not verify bytes, freshness or quality.

The top-row **Branch** menu searches available branches and applies a selection
immediately. It defaults to `master` and lists branches with heads for visible local
datasets, plus the selected branch (all workspace branches when the view is empty).
The adjacent **Fallback branches** dialog adds, reorders and removes the view-local
tail. Empty means no fallback; **Use workspace defaults** clears the override.
The server normalizes it and binds it into read context fingerprints. Frozen
versions/plans reject overrides. Workspace configuration is unchanged. Build controls
are still a later task; they must pass this override explicitly when implemented.

Drag the canvas to pan, Shift-drag for box selection, and Shift/Cmd/Ctrl-click to select multiple nodes. Drag nodes or use their arrow keys to change positions. There is no pin/unpin state. Delete/Backspace removes only focused graph selections from the view. Persisted positions and visual undo/redo are delivered in T084. Context fingerprints fence requests and reset the graph when the context changes. The worker foundation remains bundled and production-tested; it is not invoked by graph membership changes.

### UI review iteration: catalogue and canvas navigation

The right catalogue has a single debounced fuzzy search input. Case-insensitive ordered-subsequence matches are highlighted; a green dot identifies nodes already in the lineage. Results are bounded and paginated across the entire catalogue. Clicking a result adds/selects it without moving the camera or reloading the workspace.

Drag the canvas to pan; hold Shift while dragging for box selection. Delete or Backspace removes selected nodes only when the diagram has focus, never while typing in catalogue search. Fit view and fit selection sit under the zoom buttons. Other graph actions float at the upper left. Node handles are visually hidden. Hover cards stay absent until the requested metadata is ready and the hover/focus is still active.

The current catalogue page warms a context-owned lineage cache (at most 100 requested
paths per lookup; at most 2,000 cached nodes and 10,000 incoming declarations).
Lookup/expansion responses include every incoming dependency of their returned nodes,
including hidden parents. Rendering shows only edges whose endpoints are visible.
Cached additions and complete cached neighbourhoods render immediately; first uncached
reads still wait for the coordinator. A partially cached neighbour set is never treated
as complete. Context changes/disconnection discard the cache and fence late responses.
No producer imports or data copies are needed for these metadata reads.

New parents are placed to the left and children to the right, including successive
levels, without moving existing nodes or the camera. Cmd+A (Ctrl+A on Windows/Linux)
selects all visible nodes when the graph has focus. The box-selection overlay is removed
on release while node selection remains. A direct graph selection supersedes any
pending catalogue-selection animation frame. Search inputs keep native text shortcuts.

The upper-right icon toggles a legend below it. The adjacent selector defaults to
**Resource Type** (Polars Transform, SQL Transform, External Dataset, Dataset, Unknown);
**Publication** shows Published, Not built on these branches, or Unknown. Colour does
not replace the missing-publication border or other status evidence. Transform nodes
use the ƒ symbol; external nodes use a compact arrow. The React Flow attribution badge
is hidden through its supported option; its MIT license remains in the dependency.
Progress/messages are centred at the bottom; no startup text covers the graph toolbar.

**Build status** uses “No build on branch” when local branch facts confirm no
attempt, including when data is available from a fallback. Missing, foreign or
historical build metadata stays Unknown. Dashed borders indicate no publication
across the selected branch and its fallbacks; fallback data keeps a solid border.

## Saved lineages (T084)

The header's **Save** button is disabled until the lineage changes, including node
movement, membership, colour, description or branch settings. Camera pan/zoom is
transient and never enables Save or appears in saved documents. Opening a lineage
centres its saved node bounds at 100% zoom. Its adjacent
menu offers **Save as**, **Open lineage** and separate **Export SVG/PNG/JSON** actions.
Save as creates an independent document with a new name; cancelling leaves the
current document untouched. The saved name appears beside the transflow logo;
a pencil edits its single-line description below. Description is absent from save
panels. Revision conflicts preserve the draft; opening another lineage asks before
discarding unsaved changes. The legend uses a keyboard-operable custom menu below
its trigger, aligned with the legend toggle.

Saved lineages always follow the selected branch tip and ordered fallbacks; there
is no source-mode selector. Groups, notes, saved catalogue filters and fixed-source
browsing have been removed. Existing pre-release documents remain readable: the
reader omits these retired fields, converts descriptions to one line, and keeps the
ID/revision and original stored bytes until an explicit save. Saving rewrites the
reduced document using the existing revision check. Legacy UUID links still open
with authentication; the UI does not offer a share-link action.

Undo/redo buttons and graph-focused Cmd/Ctrl+Z (Shift for redo) restore membership,
selection, positions, colour and camera only, with 50 prior visual snapshots.
No view action changes builds, publications, catalogue entries or schedules.
Additions preserve positions/camera; Layout remains a disabled placeholder.

Exports use selected model fields, never page screenshots. Labels default on and
metadata off. JSON/SVG can include metadata; PNG contains rendered content only
and scales to at most 4096 pixels per side. SVG text is escaped, with no remote
resources. Previews and download links use locally generated bytes. Prepare again
after edits to update an export.

Documents are bounded to 500 datasets, finite coordinates within ±1,000,000,
zoom 0.05–2 and 1 MiB. Lists page 100 summaries; reopening resolves stable dataset
identities in bounded context-owned reads. Missing identities fail explicitly;
there is no silent truncation. Branch following does not promise historical bytes
or complete G2/G3 qualification: advanced health
inspectors remain later work.

## Dataset inspection (T085)

The right **Properties** panel follows the selected graph or catalogue dataset, starting directly with dataset identity and the About/Columns tabs.
Catalogue loads 50 matching datasets at a time and appends the next page near the
bottom on vertical scroll. Search and branch changes start a fresh result list;
the dataset count shows how many matches are loaded. Paging buttons are removed.
Failed page reads preserve loaded entries and offer an explicit retry.
About displays stable ID, logical path, origin, transform type, producer location,
creation time when recorded, publication time and physical row/file/byte counts.
The compact detail list has no row dividers. Below Connections, Freshness shows
the backend's data-only publication freshness. Data health repeats the dedicated
Health panel's ten newest-first output-check circles and latest status pill,
without clickable circles or details. Input failures remain separate from
published output quality; the connection hint is omitted.
Columns lists the current preview
version's logical schema with types and nullability. Missing metadata is labelled
unknown or unavailable; a real zero stays zero. Description and data-health remain
explicitly unavailable where their metadata is absent. With no graph node selected
or multiple nodes selected, Properties and every bottom inspector tab show a
centered “Select a node to view information” notice with an info icon, without
retaining the previous single dataset's content. Clicking empty canvas clears
Preview's table; selecting a node restores its content. Plain-clicking a node in
a multi-selection selects that node alone; Shift/Cmd/Ctrl-click can change group
membership.

The bottom **Preview** tab contains a compact dataset row and the table immediately
below it. The row shows publication time and row/column counts. If the selected
branch lacks data, a blue warning above the row names the branch supplying it.
The preview uses the selected
branch's head, ordered configured fallbacks, then an available retained branch for
browsing if none has a head. That last choice never changes build fallback policy.
Each read binds an exact immutable version and requests the available schema columns
within the coordinator's 128-column limit. Preview requests 500 physical rows per
page, subject to the coordinator's byte/row limits and provider leases. Scrolling
near the bottom automatically appends the next cursor page without a load-more
button or footer row. Only one page request runs at a time; a failed page keeps
the displayed rows and offers **Restart preview** instead of retrying on scroll.
The UI displays at most
1,000 rows across those pages. Its MIT-licensed RevoGrid Community component is
read-only and virtualized; headers preserve schema field-name case and fixed-width
columns leave an unruled blank area at the right, matching the space below the rows;
selection highlighting also ends at the final selected cell.
Click selects a cell, drag selects a rectangular range, and the top-left corner
selects all displayed cells. Right-click offers **Copy with headers** for the
existing selection, even when the pointer is outside it; Cmd+C on macOS or
Ctrl+C elsewhere copies the selection with headers too. Copied TSV escapes separators and leading
spreadsheet formula characters. Clicking a cell never copies it. Sorting, filtering
and column statistics await dataset-wide query requirements; the grid does not
sort or filter only the loaded preview rows. Column types remain in tooltips rather
than a second header line. Typed null, truncated cells, decimal precision,
timestamp units and nested values remain distinct. An unavailable version,
missing schema or failed read produces an explicit state; no page rebinds to a newer
head. Foreign manifests supply metadata counts, while provider byte availability and
freshness remain separate and unverified. The preview does not claim full artifact
integrity or data-health PASS.

The pinned RevoGrid component styles are bundled into the same-origin CSS asset
for the coordinator's content policy. TypeScript checks application source
strictly; `skipLibCheck` excludes errors in the grid package's published declaration
files (`lodash` and `@type` imports).

UI focus and node selection use thin, muted teal outlines (with a lighter dark-theme
variant). Custom menu rows separate icons, labels and checkmarks, with consistent
height and inset focus styling that cannot overlap adjacent rows.

The Open lineage dialog has fuzzy name search across bounded saved-lineage pages,
highlighted matches, version labels and committed last-save timestamps. UI dates
use `YYYY-MM-DD HH:mm:ss` in the browser's local time. Legend rows select matching
visible nodes; dependency edges share one colour regardless of origin or role.
Pointer-down on a node dismisses its tooltip until a fresh hover/focus.

## Build planning (T087)

Open **Build planner** in the right rail after selecting local producer nodes on the
lineage. No selection shows a centered hammer and selection prompt. Targets always
follow graph selection; external/imported nodes cannot become local build targets.
Choose **Selected resources only** (the UI default), **All transforms in between
selected resources**, or **All ancestor resources**. Connecting scope includes the
selected endpoints and eligible paths between them in the complete validated graph;
it excludes unrelated side ancestors, respects branch/read/pin barriers, and keeps
isolated selections as explicit targets. The shared CLI/API mode is `connecting`;
CLI's omitted mode remains `full`, and explicit `between` read-boundary semantics
remain available through CLI/API, along with advanced boundaries, exclusions, source
refresh, exact pins, parameters, require-current inputs and source refs. These advanced
fields are not shown in the inspector.

**Next (View preview)** does not execute producers or register outputs. It shows the
planned build datasets only; reused input boundaries remain in collapsed Plan details.
Force, selection/context changes and
expiry automatically prepare a new guarded preview; transient conflicts refresh context
and retry up to three times with backoff. Preparation errors remain visible with an
explicit retry; builds always require a separate Run build click.
Deterministic jobs show **Cache evaluation pending** until execution resolves actual
inputs and verifies retained bytes, so counts describe jobs in scope, not a speculative
number of materializations. The graph temporarily highlights writes and fades other
nodes. **Add to graph** adds hidden registered resources in batches of up to 100 without
moving existing nodes. **Select nodes on graph** adds/selects up to 100 planned resources
and returns to strategy selection. Pending registrations become browsable on acceptance.
Collapsed **Plan details** retains checks, exact reads, symbolic inputs, parameters,
source decisions and freshness reasons. Empty/invalid/expired/conflicting plans cannot run.

**Run build** sends only the reviewed plan ID, matching context and durable idempotency
receipt. Interrupted acceptance retries that receipt rather than creating another build.
The live Build report opens automatically; changing the next selection or canceling
its preview does not cancel an accepted build. Event refresh retains the last verified
workspace through publication conflicts with capped, cancellation-aware backoff.
The same-branch canvas keeps its membership, positions, selection and camera.

## Schedule inspector (T098)

Schedules shows related definitions for selected nodes, or for the visible graph when
nothing is selected. **All workspace schedules** is an explicit alternative. Target
and triggered-by membership remain separate, including many-to-many relationships.
Selection does not change the executable graph, saved camera or node positions.

The editor supports typed nested conditions, next intended times with timezone
offsets, targets/scope, independent source/data branches and frozen fallback policies.
Advanced force, retries, abort, overlap, expiry and missed-tick bounds are explicit.
Save replaces the current snapshot under its original ETag. Exit/Cancel offers Save and
exit, Discard changes or Keep editing when the draft is dirty. Successful schedule writes
refresh the verified runtime context before later graph/branch metadata reads.
An edit conflict keeps the draft and offers an explicit reload. Manual Run now leaves
paused state unchanged. Pause/Resume and confirmed deletion use guarded API actions.

Saved target/trigger/exclusion/boundary facts come from the backend, with bounded
pagination and edit/source/policy fences. Runtime job transitions do not invalidate
unchanged saved role facts. Hidden relevant datasets can be added explicitly.
The sidebar has no scope-preview action or persistent complete-scope notice. Irrelevant
Unknown labels are hidden while a schedule is active. Temporary role coloring leaves
the saved coloring and geometry intact.

The sidebar keeps browsing controls in the list and provides a blue Schedules back
button above the heading in selected details. View schedule aligns with the name and
description; detail labels have focus/hover help and expiry uses hours/minutes/seconds.
Dataset cards and aligned rows show compact information;
branch/fallback precedes a separate Code and build options section. **View schedule** opens a large dedicated modal
with the full current definition, code-source explanation, summary and recent builds.
Each build entry opens its retained Build report, and closing it returns to the Schedule
view. Editor Save schedule precedes Cancel at the top while scrolling. The lineage
title/description are hidden during schedule editing and restored on exit; graph role
buttons center their icons and text with consistent spacing. These views preserve the
foreground lineage selection, node positions and camera.

History/summary use persisted occurrences, exact metrics and missing timing evidence;
active histories poll without manual refresh. Schedule build reports link back to the
current schedule without changing the foreground branch or dataset selection.
See the [schedule guide](../crates/transflow/SCHEDULES.md).

In Schedule edit mode, the bottom canvas status strip exposes **Exit schedule**. A
selection toolbar offers **Target**, **Trigger** and **Clear**. The fixed-height target
list contains only schedule targets and offers **Add selected nodes**; external or
non-producer nodes cannot become targets. Actions update the draft with qualified
identities. New graph triggers are publication leaves on the draft data branch with
exact event pins; they append to an existing AND/OR group or form an AND with an
existing leaf. Clear removes matching dataset membership and normalizes empty/single
condition groups, preserving untouched leaf IDs and policies. Backend validation still
owns target eligibility, cycles and acceptance. Graph selection/roles do not rewrite
producer dependencies, canvas positions or camera.

The data-branch and fallback pickers fetch every live workspace branch page, exclude
deleted names, search the complete result and render at most 200 results per page.
Repeated cursors, failed pages and excessive traversal fail explicitly rather than
showing an incomplete successful list. Ordered fallbacks support drag, up/down and
removal. These controls change the schedule draft without switching the foreground
lineage branch or input defaults. Section separators replace outer target/condition
borders; source/input-default reset and build-scope preview buttons are omitted.

### Schedule scope and live state refinement

Selecting a saved schedule automatically requests its informational backend plan.
The sidebar and dedicated modal list every planned write, with the complete count
above the cards; the graph labels connecting jobs Attempt to build. The same saved
source, output branch, fallback/input policies and scope rules used by dispatch
apply. Retained reads/exclusions are separate roles, not jobs. Actual occurrence
pins, source refresh and cache decisions can reuse inputs instead of executing them.
Failed/aborted planning never substitutes the target list or a zero count. Late
responses cannot repaint a different schedule; runtime polling alone does not
repeat planning. Hidden planned jobs join the explicit Add-to-view action.

Paused has a pause glyph, Active a green tick, and Active and currently running
blue rotating arrows. Live state comes from every linked build in SQLite, rather
than the newest history page or a client timer. A queued request is not running.
Manual/previously started work can run while automatic dispatch remains paused;
that distinction is stated below the running label. Reduced motion stops rotation.
The View schedule arrow is 14 pixels; configuration rows sit below Branches.
The selected schedule's blue back button touches the sidebar's top and both edges.
View schedule is a compact control beside the larger schedule name; description
sits beneath that row. New schedule's icon and the list's state/branch labels are
centered, with space instead of a dot between state and branch. Compound conditions
retain All/Any nesting and use individual cards. Trigger takes precedence over a
planned-write label, with both flags still available in the role details.

### Role palette, readable conditions and Connecting build

Target uses green, Trigger amber and Attempt to build blue. Node fills exactly
match the legend in both themes, including when a plan preview is active. The
primary role remains a visible label; its hover and accessible description retain
simultaneous roles. Role-only info circles are omitted. The extra-info badge
mechanism remains available for evidence beyond the legend.

Read-only time cards describe the five numeric cron fields in plain language,
keeping the timezone. Calendar day and weekday constraints retain their OR rule.
The editor keeps raw cron and backend-computed future times; the full view also
explains how repeated local times are handled. This wording never computes or
changes fire times. Incomplete drafts and conditions without a clock do not
request a misleading whole-definition clock preview.

New UI drafts default to **Selected datasets only**. Add targets on the graph;
manual and time conditions do not imply a connecting path. **Connecting build**
becomes available with at least one distinct dataset trigger and target. It reuses
published trigger datasets as read boundaries and builds eligible producers along
the paths to the targets. Side inputs remain read boundaries. Changing graph
roles or condition leaves keeps those boundaries synchronized; incomplete enabled
connections cannot save and can always be turned off.

The option encodes the existing shared Between scope, preserving explicit target
identities. The planner uses the full captured graph and frozen branch/input
policies, not just visible nodes. Existing saved full, explicit-boundary and
selected-endpoint scopes remain available under Advanced options. Changing this
UI default does not rewrite existing definitions or accepted work, nor change the
API default or any transport/storage format.

### Compact exploration controls

Legends show category rows and their counts without a totals footer. History keeps
Summary, right-aligned metrics and bottom-aligned date controls on one wrapping
row. The measured/missing metric and chart caption are omitted; chart tooltips
retain sample scope and dates, and the axis retains duration units.
