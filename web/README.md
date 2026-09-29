# Transflow workspace and lineage

T082 connects the original React interface to a local coordinator. It provides
bounded catalogue pages, requested branch/fallback metadata, dataset and exact
version selection, explicit retained/historical source context, connection states,
light/dark themes and resizable right/bottom panels. Tabs, splitters and selection
work by keyboard. Narrow screens stack panels; layout survives context changes
within the page session.

T083 adds the React Flow dataset graph, keyboard graph controls, typed data/validation
edges, foreign boundaries and stable manual positioning. An off-thread ELK layout foundation is retained for future explicit layout. Saved views, full
preview/code/history inspectors and build/schedule actions remain later tasks. The shell does not fetch rows or source
text for display. Selecting a version cannot run a build or change dataset heads.
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

Drag the canvas to pan, Shift-drag for box selection, and Shift/Cmd/Ctrl-click to select multiple nodes. Drag nodes or use their arrow keys to change positions. There is no pin/unpin state. Delete/Backspace removes only focused graph selections from the view. Groups, persisted positions, annotations and undo/redo remain T084. Context fingerprints fence requests and reset the graph when the context changes. The worker foundation remains bundled and production-tested; it is not invoked by graph membership changes.

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
