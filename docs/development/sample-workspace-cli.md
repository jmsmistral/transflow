# Sample workspace CLI checks

Use this guide to continue manual checks in the existing **Operational alpha
review** sample at `~/dev/transflow-workspace/preview-scroll-review`.
It assumes the implementation checkout is `~/dev/transflow`, with its CLI and
sample Python environment already prepared. For a new installation, start with
[Getting started](../getting-started.md).

## Start here: commands without `--workspace`

Run these in each terminal you use for CLI checks:

```bash
export PATH="$HOME/dev/transflow/target/debug:$PATH"
cd "$HOME/dev/transflow-workspace/preview-scroll-review"

command -v transflow
transflow --version
transflow catalog list
```

`command -v` should resolve to your checkout's `target/debug/transflow`. Adding
that directory to PATH lets you type `transflow` directly. Changing directory
selects the sample: Transflow finds the nearest ancestor containing
`workspace.toml`, so `--workspace` is unnecessary here or inside `src/`.
The sample's existing configuration selects its prepared Python environment;
there is no need to activate the implementation's Python tooling environment.

For Tab completion of commands, options and fixed option values, enable this in
the same Bash terminal:

```bash
source <(transflow completions bash)
```

See [Bash completion](../bash-completion.md) to keep it enabled in new terminals.
Dataset and branch names are still entered explicitly.

If the CLI binary is missing or needs rebuilding:

```bash
(cd "$HOME/dev/transflow" && cargo build --locked --offline -p transflow)
```

The PATH change applies to this terminal session. If a new terminal says
`transflow: command not found`, repeat the setup block. If you run commands from
the implementation checkout instead, you have left the sample's workspace.

## What is in this sample?

| Dataset | Role in the pipeline |
|---|---|
| `raw/orders` | Generates 1,500 synthetic orders, with an output quality check. |
| `curated/east` | Python factory selecting the east orders. |
| `curated/west` | Metadata-aware Python factory selecting the west orders. |
| `curated/delivery` | Combines the regional datasets. |
| `analytics/region_summary` | Generated SQL producing per-region counts, totals and averages. |

The lineage is `raw/orders → east/west → delivery → region_summary`.
The workspace has `master` and `review` data branches. `master` has the complete
pipeline; `review` starts with orders and can read other inputs through its
configured `master` fallback. Commands below use `--branch master` where relevant
so that their destination is explicit.

## Inspect the catalogue and retained history

```bash
transflow catalog list
transflow catalog show curated/delivery
transflow branch list
transflow build list --limit 10
transflow schedule list
```

These inspect retained state. Catalogue producer locations can be marked stale;
validation/planning refreshes source evidence. For machine-readable output:

```bash
transflow --json catalog show curated/delivery
```

Choose an ID from the build or schedule list, then replace the uppercase
placeholder in the relevant command:

```bash
transflow build show BUILD_ID
transflow build logs BUILD_ID
transflow schedule show SCHEDULE_ID
transflow schedule history SCHEDULE_ID --limit 10
```

## Inspect dependencies and preview build scope

```bash
transflow upstream curated/delivery --branch master
transflow downstream raw/orders --branch master
transflow why curated/delivery --branch master

transflow plan curated/delivery --branch master --mode selected
transflow plan curated/delivery --branch master --mode full
transflow plan curated/delivery --branch master --mode between --boundary raw/orders
```

| Preview | Eligible datasets in this sample |
|---|---|
| Selected delivery | Delivery only; use published east/west inputs. |
| Full delivery | Orders, east, west and delivery. |
| Between orders and delivery | East, west and delivery; use published orders. |

Planning captures and validates declarations and can retain a guarded plan and
read leases. It does not execute producer functions or publish datasets.
`why` explains freshness and prospective selection. Both default to full scope;
`build` defaults to selected scope, so use an explicit mode when comparing them.

The schedule editor's **Connecting build** option uses Between scope with its
trigger datasets as boundaries. The CLI's separate `--mode connecting` connects
selected target endpoints; use the Between command above to reproduce the
trigger-to-target schedule example. See [Schedules](../../crates/transflow/SCHEDULES.md).

## Run builds when ready

These commands execute work and may publish new versions and history:

```bash
transflow build curated/delivery --branch master --mode selected
transflow build analytics/region_summary --branch master --mode selected
```

The first builds delivery from published regional inputs. The second builds the
summary from published delivery. To include all upstream datasets in one build:

```bash
transflow build analytics/region_summary --branch master --mode full
```

To request execution throughout that scope even when results are reusable:

```bash
transflow build analytics/region_summary --branch master --mode full --force
```

Without force, reuse and source-refresh policies determine which eligible jobs
execute. A repeated build need not execute every producer. Required check
failures preserve the previous successful published output.

Inspect the returned build ID with `build show` and `build logs`. Ordinary builds
wait for completion and can run without a separately started coordinator.

## Keep the UI open alongside CLI checks

In terminal A, start the existing sample launcher:

```bash
bash "$HOME/dev/transflow-workspace/preview-scroll-review-tools/serve-preview.sh"
```

It rebuilds the checkout's CLI/UI and opens the authenticated browser. Stop any
older coordinator for this same workspace first. Open **Lineage actions → Open
lineage → Operational alpha review** if the canvas starts empty.

In terminal B, repeat the PATH and `cd` setup above, then run CLI commands. The
running coordinator handles builds; inspect their results in History, Preview,
Code and the Build report. Stop terminal A with Ctrl-C when finished.

With existing artifacts, the equivalent direct CLI launch from the sample
workspace is:

```bash
transflow serve --ui-dir "$HOME/dev/transflow/web/dist" --open
```

For asynchronous submission and cancellation while that coordinator is running:

```bash
transflow build analytics/region_summary --branch master --mode full --force --no-wait
transflow build show BUILD_ID
transflow build cancel BUILD_ID
```

Replace `BUILD_ID` with the returned ID. This small sample can finish before a
cancellation reaches it. Ctrl-C in a log-following client only stops that observer;
`build cancel` is the explicit cancellation command.

## Check a saved schedule

With the coordinator running, choose an ID from `schedule list`:

```bash
transflow schedule show SCHEDULE_ID
transflow schedule run SCHEDULE_ID
transflow schedule history SCHEDULE_ID --limit 10
```

Run submits the saved definition and does not unpause it. Inspect its saved
source, branch, targets and conditions first: a saved source snapshot can differ
from the current files that an ordinary CLI build captures.

In the UI, a schedule uses **Trigger**, **Target**, **Not built**, and **Attempt to
build** for intermediate nodes when Connecting build is on. Toggle it in Edit to
check the live preview, then Cancel to preserve the saved definition.

For more options, see [builds](../../crates/transflow/BUILDS.md),
[planning and inspection](../../crates/transflow/INSPECTION.md),
[schedules](../../crates/transflow/SCHEDULES.md) and the
[lineage UI reference](../../web/README.md).
