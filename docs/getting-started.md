# Getting started with Transflow

This guide takes the current developer preview from a source checkout to a real
customer-orders build and the local lineage UI. The example data is synthetic.
The native CLI, matching Python wheel and web assets are built separately today.

## 1. Prepare the source tools

You need Git, Python **3.14**, the Rust toolchain pinned in
[`rust-toolchain.toml`](../rust-toolchain.toml), Node **24.4.1** and npm **11.4.2**.
Ensure `python --version` reports Python 3.14 before creating the tooling environment.
Dependency downloads below are explicit setup; ordinary dataset builds do not
install packages.

```bash
git clone https://github.com/jmsmistral/transflow.git
cd transflow

cargo fetch --locked
cargo build --locked --offline -p transflow
npm --prefix web ci --ignore-scripts
npm --prefix web run prepare:types
npm --prefix web run build

python -m venv target/python/py314
target/python/py314/bin/python -m pip install \
  --require-hashes --only-binary=:all: -r python/dev-py314.lock
target/python/py314/bin/python -m pip wheel \
  --no-index --no-deps --no-build-isolation \
  --wheel-dir target/quickstart-wheels ./python
```

The locked tooling environment includes the package builder and environment-locking
tools. It is separate from each workspace's managed execution environment. See the
[environment guide](../crates/tf-exec/ENVIRONMENTS.md) for offline wheelhouses,
platform-specific locks and configuration.

Keep this shell open, or reset the paths below when returning to the guide.

```bash
transflow_repo="$PWD"
tooling_python="$transflow_repo/target/python/py314/bin/python"
export PATH="$transflow_repo/target/debug:$PATH"
```

## 2. Create an example workspace

Choose a new directory. Initialization preserves existing files and refuses
conflicting state. The catalogue can be committed; runtime data stays ignored.

```bash
demo_workspace="$HOME/transflow-demo"
transflow init "$demo_workspace"
cp -R examples/customer-orders/src/. "$demo_workspace/src/"

transflow --workspace "$demo_workspace" env lock --python "$tooling_python"
transflow --workspace "$demo_workspace" env sync \
  --python "$tooling_python" \
  --runtime-wheel "$transflow_repo/target/quickstart-wheels/transflow-0.0.0.dev0-py3-none-any.whl"
```

Keep the same tooling interpreter available for UI-initiated discovery/builds by
writing the local interpreter setting. The small script quotes the absolute path
as a TOML string, including spaces, and refuses to overwrite an existing setting.

```bash
"$tooling_python" - "$demo_workspace" "$tooling_python" <<'PY'
import json
import pathlib
import sys

config = pathlib.Path(sys.argv[1]) / "transflow.local.toml"
with config.open("x") as stream:
    stream.write("[python]\nexecutable = " + json.dumps(sys.argv[2]) + "\n")
PY
```

The bundled Python distribution contains both the SDK (`transflow`) and the worker
(`transflow_worker`). Use the locally built matching wheel; this project has not
published a public release. No `pip install transflow` step is needed for this guide.

## 3. Build and inspect

```bash
transflow --workspace "$demo_workspace" build curated/customer_orders
transflow --workspace "$demo_workspace" catalog show curated/customer_orders
transflow --workspace "$demo_workspace" plan curated/customer_orders
```

The build registers two source datasets and their joined output, executes required
checks, and publishes immutable Parquet versions. The output contains the synthetic
customer orders described in the [walkthrough](../examples/customer-orders/README.md).
A failed required check leaves the previous successful output available.

The source producers in this example refresh on each full build. To build just the
joined output against the current source versions:

```bash
transflow --workspace "$demo_workspace" build curated/customer_orders --mode selected
```

## 4. Open the lineage UI

From the repository checkout:

```bash
bash tools/serve-review.sh "$demo_workspace" --open
```

The launcher builds this checkout's CLI and production UI, starts the coordinator
for the selected workspace and opens an authenticated browser page. Search for
`curated/customer_orders` in Catalogue, add it to the graph and expand upstream.
Select a dataset to explore Preview, SQL scratchpad, History and Code. Save the
lineage to return to that graph later.

Stop the foreground coordinator with Ctrl-C. After pulling UI changes, stop the
old coordinator and launch again: it serves the asset snapshot from its startup.
Opening a new browser page uses a fresh authenticated launch grant.

For a prepared multi-feature workspace with schedules, two data branches and build
history, follow the [operational demo](../examples/operational-alpha/README.md).
The [documentation index](README.md) links to authoring and operational references.
