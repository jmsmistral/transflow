# Transflow workspace and lineage

T082 connects the original React interface to a local coordinator. It provides
bounded catalogue pages, requested branch/fallback metadata, dataset and exact
version selection, explicit retained/historical source context, connection states,
light/dark themes and resizable right/bottom panels. Tabs, splitters and selection
work by keyboard. Narrow screens stack panels; layout survives context changes
within the page session.

T083 adds the React Flow dataset graph, keyboard graph controls, typed data/validation
edges, foreign boundaries and stable manual positioning. An off-thread ELK layout foundation is retained for future explicit layout. T084 adds saved views (below). T085 adds properties, columns and exact-version preview (below). Code/history and build/schedule actions remain later tasks. Selecting a version cannot run a build or change dataset heads.
Metadata availability is not byte verification or freshness evidence.

## Build and connect

Use Node 24.4.1/npm 11.4.2 and the exact lockfile. Installation is explicit:

```bash
npm --prefix web ci --ignore-scripts
npm --prefix web run prepare:types
bash tools/check-web.sh
npm --prefix web run build
cargo build --locked --offline -p transflow
target/debug/transflow --workspace /path/to/workspace serve --ui-dir "$PWD/web/dist" --open
```

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
or seamless live-build preview updates: code/history and build/schedule/health
inspectors remain later work.

## Dataset inspection (T085)

The right **Properties** panel follows the selected graph or catalogue dataset.
About displays stable ID, logical path, origin, transform type, producer location,
creation time when recorded, publication time and physical row/file/byte counts.
The compact detail list has no row dividers. Columns lists the current preview
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
within the coordinator's 128-column limit. Pages are bounded to 100 physical rows,
with the coordinator's cursor, limits and provider leases. The UI displays at most
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
complete planned resource list and faded reused boundaries; toggle their visibility
with **Show resources that will not be built**. Force, selection/context changes and
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
