# SQL and checks review

This synthetic fixture builds `raw/orders` and `raw/customers`, produces
`curated/qualified_orders` with SQL, and consumes it in the Polars
`curated/report` producer. SQL binds an exact decimal parameter. Its checks use
two aliases for the customer dataset: three customers on master and one on review.
These validation-only inputs do not become SQL views or Python function arguments.

## Create a fresh workspace

Use the prepared Python 3.14 resolver tooling, matched CLI/wheel and offline
wheelhouse. Creation refuses an existing workspace.

```bash
target/python/py314/bin/python -m pip wheel --no-index --no-deps \
  --no-build-isolation --wheel-dir target/sql-checks/wheels ./python
target/python/py314/bin/python -I -B python/tools/create_sql_checks_demo.py \
  ~/dev/transflow-workspace/t104-sql-checks-review \
  --cli target/debug/transflow \
  --wheel target/sql-checks/wheels/transflow-0.0.0.dev0-py3-none-any.whl \
  --wheelhouse target/qualification/wheelhouse
target/debug/transflow --workspace ~/dev/transflow-workspace/t104-sql-checks-review \
  serve --ui-dir "$PWD/web/dist" --open
```

Open the saved **SQL and checks review** view. Inspect qualified orders' Preview,
Code, History and Data health. Code shows the captured `.sql` file. The initial
build executes producers and the repeat is cached without fabricated attempts.
Build reports show the exact jobs and published quality evidence. Both schedules
start paused; Run now is explicit.

The review branch contains customers only. Select master to inspect the complete
pipeline. View-local fallbacks affect browsing; they do not rewrite the schedule
or producer's explicit input branches.

## Automated lifecycle checks

```bash
target/python/py314/bin/python -I -B python/tools/check_sql_checks_demo.py \
  target/debug/transflow \
  target/sql-checks/wheels/transflow-0.0.0.dev0-py3-none-any.whl \
  target/qualification/wheelhouse --output target/sql-checks-lifecycle.json
```

This uses a disposable workspace. It checks setup refusal, SQL-only invalidation,
failed-output head preservation, blocked descendants, cache reuse, exact alias
provenance and retained SQL replay without changing current source. It leaves the
persistent review workspace unchanged.
