# Transflow web foundation

T006 provides a strict TypeScript/React application, accessible native controls,
component tests and reproducible Vite assets. The development preview shows an
unconnected workspace with a session-local light/dark theme and an information
dialog. It makes no coordinator requests. Dataset operations, the application
context shell (T082) and interactive lineage graph (T083) remain unimplemented.

The [canonical UI specification](../../transflow-spec/TECHNICAL_ARCHITECTURE.md#16-lineage-ui-specification)
defines the intended application. Private screenshot references stay in the
ignored specification directory; the preview uses original inline SVG and system
fonts. No reference screenshots, credentials or remote assets are bundled.

## Setup and commands

Use Node 24.4.1 from the root `.node-version` and npm 11.4.2. Dependencies are exact
pins in `package.json`, resolved with integrity hashes in `package-lock.json`.
Installation is explicit; the checks do not install dependencies or browsers.

```bash
cd ~/dev/transflow
npm --prefix web ci --ignore-scripts
bash tools/check-web.sh
npm --prefix web run dev
```

The development server binds to loopback. To inspect the production assets:

```bash
npm --prefix web run build
npm --prefix web run preview -- --port 4173 --strictPort
```

Open `http://127.0.0.1:4173/` in Codex's internal Browser and follow the
[browser checklist](../docs/development/browser-checks.md). Browser verification
uses the internal Browser only: do not install Playwright, browser drivers or
browser binaries. Vitest's jsdom component tests need no browser installation.

`tools/check-web.sh` checks exact Node/npm versions, strict types, ESLint (including
React hooks and accessibility rules), Prettier, six component tests and two
production builds. It compares every asset byte and writes component results and
SHA-256 hashes under ignored `target/web/`. The [web CI workflow](../.github/workflows/web.yml)
runs these gates on macOS arm64 and Linux x86_64/arm64 without browser installs.
The internal Browser checklist is a separate observed check, not an automated CI
browser or full accessibility audit.

## Boundaries and dependencies

`src/components.tsx` contains native buttons, a labelled modal dialog, textual
status and a safe render-error fallback. Keyboard focus is visible, the skip link
moves focus to the workspace and status is never conveyed by colour alone.
`src/api/` reserves the generated-contract boundary. No hand-written domain types,
coordinator client or provisional API fixtures are introduced; generation follows
the Rust schemas in T012/T074. React Flow and ELK remain in the T003 probe until
needed by T083.

The app uses the qualified React 19.3.0, TypeScript 6.0.3 and Vite 8.3.0 baseline.
jsdom 26.1.0 supports the pinned Node version; the latest jsdom requires a newer
Node patch. ESLint 9.39.5 is retained for the accessibility plugin's supported
peer range. npm reports that ESLint version as unsupported and also deprecates
jsdom's transitive `whatwg-encoding`; revisit these development-tool pins during
T008's dependency audit. This is not a clean dependency-audit result.

`dist/` contains relative local asset URLs and a Vite manifest for later Rust
embedding (T117). Repeatability is measured across two local builds with the same
lock/toolchain, not across platforms. Node is a development tool; native asset
embedding and offline release verification have not yet been implemented.
