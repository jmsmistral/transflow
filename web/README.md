# Transflow workspace and lineage

T082 connects the original React interface to a local coordinator. It provides
bounded catalogue pages, requested branch/fallback metadata, dataset and exact
version selection, explicit retained/historical source context, connection states,
light/dark themes and resizable right/bottom panels. Tabs, splitters and selection
work by keyboard. Narrow screens stack panels; layout survives context changes
within the page session.

T083 adds the React Flow dataset graph, keyboard graph controls, typed data/validation
edges, foreign boundaries and an off-thread ELK layout. Saved views, full
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

Select a catalogue dataset to add its depth-zero root, or open **Explore / search**
to enter a path, search the entire paginated catalogue, or find visible nodes.
The canvas fills the available workspace below a compact toolbar. Both inspectors
start collapsed, with right-side icons and bottom tabs beside expand/retract buttons.
Search/layout controls open over the canvas; counts and the legend are collapsible.

Compact nodes contain `<` / `>` handles inside their ends, only where neighbours
exist. Hover labels give the total unique count from the complete captured graph.
Clicking drains bounded 100-node/100-edge pages for all immediate neighbours and
selects them, preserving zoom. Fully visible neighbours reverse the arrow; clicking
retract removes those neighbours and their incident edges from the view. Unrelated
nodes remain. Generic depth/path exploration still offers explicit next-page actions
and a 500-node opt-in. No automatic fitting occurs after expansion; use **Fit view**.
Node hover/focus cards lazily fetch available retained metadata. Edge text and the
separate dataset-list view have been removed; keyboard graph navigation remains.

The top-row **Branch** menu searches available branches and applies a selection
immediately. It defaults to `master` and lists branches with heads for visible local
datasets, plus the selected branch (all workspace branches when the view is empty).
The adjacent **Fallback branches** dialog adds, reorders and removes the view-local
tail. Empty means no fallback; **Use workspace defaults** clears the override.
The server normalizes it and binds it into read context fingerprints. Frozen
versions/plans reject overrides. Workspace configuration is unchanged. Build controls
are still a later task; they must pass this override explicitly when implemented.

Use Shift/Cmd/Ctrl or drag selection for multi-selection.
**Show paths** queries all declared directed paths from the first selected dataset
to the second, including initially hidden intermediate nodes; it highlights the
returned scope as pages arrive. This is a read-only graph query, not a build mode.
The server intersects forward/reverse reachability without enumerating paths.
Foreign nodes are read-only boundaries; browsing never executes provider code.
Provider provenance expansion remains available through the existing CLI.

Drag nodes or move a focused selected node with arrow keys to pin its position.
Incremental layout preserves pins, including moves made while a worker is running.
**Relayout all** clears pins; **Remove from view** changes only the view. Groups,
annotations, persisted positions and undo/redo belong to T084. Freshness/quality
remain explicitly unknown until the later overlay task. Imported/producer types
use available retained metadata; missing type information stays generic.

Context changes fence both graph requests and worker replies. Manual view state
survives dataset selection in the same frozen context, and resets for a different
fingerprint. Layout failure preserves manual graph interaction and offers retry.
ELK's native worker is bundled locally, with a same-origin worker CSP. The explicit
preparation command applies reviewed, version-checked ELK and xyflow declaration
corrections; it does not change runtime library code or disable strict checking.
Production verification executes the actual worker asset in a background thread,
checks a diamond and an invalid graph, and compares reproducible build hashes.
