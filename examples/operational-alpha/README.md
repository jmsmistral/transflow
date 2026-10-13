# Operational alpha review

This synthetic demo combines a four-dataset diamond, output quality checks,
parallel regional transforms, executed/cached history, two paused schedules,
two live data branches and a saved lineage view. All amounts are invented integers.
It creates a new workspace and refuses to overwrite an existing one.

## Create the workspace

From the implementation checkout, use the prepared Python 3.14 tooling, built CLI,
matched local wheel and offline wheelhouse:

```bash
target/python/py314/bin/python -m pip wheel --no-deps --no-build-isolation \
  --wheel-dir target/operational-alpha/wheels ./python
target/python/py314/bin/python -I -B python/tools/create_alpha_demo.py \
  ~/dev/transflow-workspace/t099-alpha-review \
  --cli target/debug/transflow \
  --wheel target/operational-alpha/wheels/transflow-0.0.0.dev0-py3-none-any.whl \
  --wheelhouse target/qualification/wheelhouse
```

Use another new directory for a fresh demo. Dependency setup is explicit and
offline; ordinary builds do not install packages. `DEMO.json` in the created
workspace records generated view, schedule and build identities.

## Open the UI

```bash
target/debug/transflow --workspace ~/dev/transflow-workspace/t099-alpha-review \
  serve --ui-dir "$PWD/web/dist" --open
```

Open **Operational alpha review** through the saved lineage menu. If starting
from an empty canvas, search `curated/delivery` in Catalogue and expand upstream.
All four nodes are synthetic. The review branch publishes orders only;
use the lineage fallback dialog to add master and inspect its regional/delivery versions.

## Review the integrated behavior

1. Select delivery and inspect Preview, Code, History and Summary. The first
   build executes four jobs; the repeat reuses data without fabricating attempts.
2. Hide an ancestor, select delivery and open Builds. Choose all ancestors and
   review the actual planned resources. Force enables every eligible producer;
   the preview lists planned producers, with guarded cache evaluation at execution.
   Run build opens the retained
   report with live phases, a smooth Gantt timeline and subsequent history.
3. Open Schedules. The two definitions are paused so opening the demo does not
   start automatic builds. View schedule opens its dedicated modal and recent
   builds. Run now is explicit and leaves the schedule paused.
4. Edit Weekday delivery. Add/remove graph targets or publication triggers,
   inspect the data branch search and ordered fallbacks, then Cancel to preserve
   the definition. Save replaces the current definition under its edit guard.
5. Switch between master/review and select a dataset through Catalogue. Preview and producing
   source stay bound to the displayed data version; missing branch data is explicit.
   Saved node positions survive reopening; pan/zoom does not mark the view dirty.
6. In SQL scratchpad, run `SELECT region, SUM(amount) FROM dataset GROUP BY region`.
   Results use the selected immutable version. File/network reads and mutation
   statements are rejected by the separate scratchpad policy.
7. Data health shows the exact published version's output checks separately
   from the latest attempted job and its consumer input checks. Cached reuse
   keeps the original check evidence and creates no new attempt.

Stop the foreground coordinator with Ctrl-C after review.

## Automated failure evidence

The disposable qualification runner also attempts repeated setup, forces a
negative amount through the output check, verifies every last-good head remains
unchanged and verifies blocked descendants. It restores source and builds again.
It does not modify the persistent review workspace:

```bash
target/python/py314/bin/python -I -B python/tools/check_alpha_demo.py \
  target/debug/transflow \
  target/operational-alpha/wheels/transflow-0.0.0.dev0-py3-none-any.whl \
  target/qualification/wheelhouse --output target/t099-demo-check.json
```
