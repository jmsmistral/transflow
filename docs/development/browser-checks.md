# Internal Browser verification

Use Codex's internal Browser for browser-based checks. Do not install Playwright,
external browser drivers or browser binaries. The automated component harness is
Vitest/jsdom; it does not prove native browser focus or layout behaviour.

For T006, build and serve the production preview as documented in
[web setup](../../web/README.md). Run this checklist through the internal Browser:

1. Open the loopback preview. Confirm the title, “Not connected” status and
   “No workspace connected” empty state. Check the visible layout and console
   errors/warnings. There should be no dataset or build success claimed.
2. From initial page focus, press Tab and Enter on “Skip to workspace”. Verify
   focus moves to the main workspace; Tab then reaches “About this preview”.
3. Open that dialog with Enter. Confirm its title and close button are accessible,
   background controls are unavailable while modal, and Tab/Shift+Tab do not
   activate them. Escape must close it and return focus to the opener. Repeat
   using the close button.
4. Use Space and Enter on the labelled theme toggle. Verify its pressed state,
   light/dark colours, legibility and visible focus. Reload after selecting dark;
   the preview returns to its default light theme (there is no persisted setting).
5. Test a 375 × 812 viewport in both themes, including the dialog. Confirm text and
   controls fit, the page can scroll to its footer and content width does not
   exceed viewport width. Restore the normal viewport after testing.
6. Record the tested build's hashes, viewport, actual observations and any failures
   in the task evidence. Do not report this as automated browser coverage, a
   screen-reader audit or full UI/coordinator conformance.

The current preview has no backend. Real-coordinator journeys, request/context
races and graph keyboard alternatives must be added with T074/T082/T083 and their
later acceptance scenarios. Do not substitute mock success screens for those
checks. Keep private reference images out of test evidence and production assets.
