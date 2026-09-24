# Customer-orders walkthrough

This synthetic example contains two source producers, one derived producer and a
shared name-cleaning helper. It demonstrates the implemented Polars CLI path with
ages 0 and 199, exact primary-key/input checks and an output primary-key check.
Amounts are integer minor units. [expected.json](expected.json) contains the exact
published rows and check phases.

Use the locally built native CLI and matched wheel; Transflow has not been
published. Prepare Python 3.14 with the qualified pip/pip-tools versions as described
in [environment setup](../../crates/tf-exec/ENVIRONMENTS.md). From the repository
root, with `transflow` on PATH:

```bash
transflow init /path/to/customer-analysis
cp -R examples/customer-orders/src/. /path/to/customer-analysis/src/
cd /path/to/customer-analysis
transflow env lock --python /path/to/prepared/python
transflow env sync --python /path/to/prepared/python --runtime-wheel /path/to/transflow-0.0.0.dev0-py3-none-any.whl
transflow build curated/customer_orders --python /path/to/prepared/python
```

For offline setup, add `--no-index --wheelhouse /path/to/wheels` to both environment
commands. Init supplies the qualified Polars requirement; lock also includes
Transflow's pinned DuckDB check engine without editing your requirements input.
The workspace environment is prepared explicitly once. Builds never install
dependencies, and no Git, trust, discovery or catalogue-sync command is required.

The first build creates exactly `raw/orders`, `raw/customers` and
`curated/customer_orders` in `.transflow/catalog.toml`. The result contains Ada's
order 1001 for 1742 and Grace's order 1002 for 9117, both dated 2026-09-18. Build
output shows the build ID; these commands inspect its evidence:

```bash
transflow catalog show curated/customer_orders
transflow build show <build-id> --json
transflow upstream curated/customer_orders --depth 1 --python /path/to/prepared/python
transflow downstream raw/orders --python /path/to/prepared/python
transflow build curated/customer_orders --mode selected --python /path/to/prepared/python
transflow build curated/customer_orders --mode selected --force --python /path/to/prepared/python
```

Selected mode pins the current source versions. An unchanged selected build reuses
the derived version and its original check timestamps, with no new attempt.
Selected force runs the join and its checks again without refreshing the sources.
Repeating the default full build **does refresh both sources**: their default
refresh policy is `always`.

For a non-Git experiment, edit the helper and build with
`--branch experiment --mode selected`. In a Git workspace, commit the source,
configuration, lock and catalogue, then switch to `feature/customer-name-cleanup`.
Edit `common/names.py` to append `.str.to_uppercase()` to the expression and build
in selected mode. The checked-out Git branch selects the new data branch
automatically. Source reads fall back to master; only the derived output gets a
feature head. Master remains unchanged. Keep `.transflow/runtime/` ignored.

To demonstrate rejection, change a customer age to 200, build `raw/customers`, then
build the derived target in selected mode. Its required input check fails before
the join runs. The source version remains published and the derived last-good
head remains available. Duplicate customer IDs behave the same way. Force still
runs required checks; it cannot override a failed output check.

Public dataset preview/history commands, `serve --open` and external registration
are not delivered yet. Use `build show` for execution/check history; the acceptance
runner reads actual retained Parquet to verify values. The current `serve` is a
headless CLI coordinator. Runtime C references resolve after registration, but
editor overlays and retained catalogue-browse graph caches currently refresh via
optional `catalog sync --python /path/to/prepared/python`, not ordinary builds.
Automatic refresh and native editor qualification remain G1 follow-up work.
Foreign-data qualification remains separate work.

## Contributor acceptance

Run after explicit toolchain/dependency setup, sequentially with other worker-heavy
suites:

```bash
target/qualification/py314/bin/python -I -B python/tools/check_developer_core.py \
  target/debug/transflow /path/to/transflow-0.0.0.dev0-py3-none-any.whl \
  target/qualification/wheelhouse --output target/qualification/developer-core.json
```

The runner uses fresh disposable workspaces for no-Git-executable and Git-feature
variants. It records CLI output, registry before/after, expected rows, branch heads
and passed assertions, including structural refusals and forced output failure.
Helper-only edits invalidate cache on an existing branch; reverting can reuse a
retained version. Structural refusals assert the specific error and source locations.
Reports survive failed assertions; scratch workspaces are removed unless
`--keep-failed` is supplied. SQLite queries use query-only connections; coordination sidecars may be created. The native
qualification workflow runs this on its three supported platforms and retains the
JSON report. A passing walkthrough does not close G1, native editor qualification
or release qualification.
