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
