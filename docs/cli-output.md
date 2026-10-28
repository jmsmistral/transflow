# Terminal output

Transflow uses headings, aligned tables and colour in an interactive terminal.
Builds show a live list of dataset jobs. Status always includes words and a symbol,
so the display remains readable without colour.

## Watch a build

From a prepared workspace:

```bash
transflow build curated/order_totals --mode full
```

An illustrative frame looks like this; durations and phases come from your run:

```text
Build 94c81e2a...   [======            ] 1/3 complete   3.4s
✓ raw/orders                 [==========] Succeeded       1.2s
◐ curated/order_totals        [  ===     ] Running         0.8s
· analytics/summary                       Waiting for inputs
```

- The circle and activity bar animate while a job is active. The bar is **not a
  percentage**: workers report phases, not how much work remains within a phase.
- The overall bar counts finished jobs, including failures and cancellations.
  It does not claim that every finished job succeeded.
- Successful and cached jobs turn green on their existing lines. Failures and
  blocked jobs are red; cancellations and retry waits are yellow.
- Phases distinguish checking inputs, running, writing data, checking output and
  publishing. Only persisted execution transitions drive these labels.
- Live elapsed times measure time observed by this CLI session. The final report
  uses retained build timestamps; `build show` includes attempt and phase times.
- Large builds prioritize active jobs within the terminal height and disclose how
  many rows are hidden. All jobs appear in the final report. Long labels are
  shortened to fit; full names and identifiers remain in detailed/JSON reports.

The display works both with standalone builds and when the CLI submits to an
already running coordinator. Ctrl-C in a waiting build requests cancellation and
restores the terminal cursor. A terminal resize redraws at the new width; the
previous frame can remain in scrollback to avoid erasing unrelated output.

Progress goes to **stderr**; the final report goes to **stdout**. This keeps the
result separate from the live display. A coordinator's own concurrent build logs
stay append-only. `--no-wait` reports acceptance, not completion.

```bash
transflow build list
transflow build show BUILD_ID
transflow build logs BUILD_ID
transflow build cancel BUILD_ID
```

## Command presentation

| Commands | Human terminal output |
|---|---|
| `init` | ASCII wordmark, installed version, workspace status and setup guidance |
| Help and version | Highlighted headings and the installed version |
| `env`, `validate`, `catalog sync` | Spaced reports with emphasized headings and labeled fields |
| `catalog list`, `branch list` | Aligned summary tables; detailed inspection retains identity and policy information |
| Catalogue/branch changes, `external`, `dataset import`, `gc` | Grouped reports retaining previews, blockers, warnings and publication context |
| `plan`, `why`, `upstream`, `downstream` | Styled inspection reports retaining scope, read boundaries and resolution details |
| `build`, `build replay` | Live jobs followed by a result summary, checks and inspection hints |
| `build list`, `build show`, `build cancel` | History table or job summaries with retained attempt/check details |
| `schedule list` | Name, ID, status and branch table |
| Other `schedule` operations | Labeled, nested fields instead of raw JSON syntax |
| `serve`, `build logs` | Append-only readiness/log output; user log text is not interpreted as a report |
| Errors | Highlighted human explanation, cause and recovery guidance |
| `completions bash`, `lsp --stdio` | Raw script or protocol bytes, never decorated |

## Colour, scripts and accessibility

```bash
transflow --color never build curated/order_totals
NO_COLOR=1 transflow build curated/order_totals
transflow --json build curated/order_totals > result.json
transflow --color always build list
```

`--color auto` is the default: colour is enabled only on a capable terminal.
Nonempty `NO_COLOR` disables automatic colour; explicit `--color always` overrides
it. `--color never` disables colour but keeps readable terminal layout and live
activity. UTF locales use circle/check glyphs; other locales use ASCII symbols.

Redirected output and `TERM=dumb` use the existing plain reports and append-only
progress. Live cursor movement requires both stdout and stderr to be terminals
and is disabled when `CI` is set. `--color always` can explicitly style redirected
human reports, but it never enables cursor animation in a pipe.

For scripts, use `--json`: one versioned envelope, no colour or progress output,
and the existing exit statuses (0 success, 1 failure, 2 usage error, 130 canceled).
Human layouts are intended for reading, not parsing. Completion scripts, LSP
frames and exported data files bypass presentation entirely.

See [build commands](../crates/transflow/BUILDS.md) and
[CLI conventions](../crates/transflow/README.md) for execution and output contracts.
