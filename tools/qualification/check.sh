#!/usr/bin/env bash
# Run already-installed, locked dependency probes. Never installs dependencies.
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
cd -- "$repo_root"
python_bin="${PYTHON:-python}"
if [[ "$(rustc --version)" != "rustc 1.98.1 "* ]]; then
    echo "Qualification requires the pinned Rust 1.98.1 toolchain." >&2
    exit 1
fi
if [[ "$(node --version)" != "v24.4.1" || "$(npm --version)" != "11.4.2" ]]; then
    echo "Qualification requires Node 24.4.1 and npm 11.4.2." >&2
    exit 1
fi
export CARGO_TARGET_DIR="$repo_root/target/qualification/cargo"
mkdir -p "$repo_root/target/qualification/runs"
run_dir="$(mktemp -d "$repo_root/target/qualification/runs/native.XXXXXX")"

cargo fmt --manifest-path tools/qualification/rust/Cargo.toml -- --check
cargo clippy --locked --offline --all-targets --manifest-path tools/qualification/rust/Cargo.toml -- -D warnings
cargo run --quiet --locked --offline --manifest-path tools/qualification/rust/Cargo.toml \
    -- --output-dir "$run_dir/data" > "$run_dir/rust.json"

# A nonempty output must fail before touching existing probe artifacts.
if "$CARGO_TARGET_DIR/debug/transflow-dependency-probe" --output-dir "$run_dir/data" > "$run_dir/repeated.stdout" 2> "$run_dir/repeated.stderr"; then
    echo "Probe incorrectly accepted a nonempty directory." >&2
    exit 1
fi
"$python_bin" -c 'import pathlib,sys; text=pathlib.Path(sys.argv[1]).read_text(); sys.exit(0 if "Probe output directory must be empty" in text else 1)' "$run_dir/repeated.stderr"

"$python_bin" -m pip check
"$python_bin" tools/qualification/python/probe.py --rust-parquet "$run_dir/data/rust.parquet" > "$run_dir/python.json"
"$python_bin" tools/qualification/duckdb/probe.py --output "$run_dir/duckdb-capabilities.json"
"$python_bin" tools/qualification/resources/probe.py --output "$run_dir/resource-capabilities.json"
"$python_bin" -I -B python/tools/check_declaration_returns.py > "$run_dir/declaration-returns.json"
cargo build --locked --offline -p tf-store --example normalization_probe
"$python_bin" -I -B python/tools/check_normalization.py "$CARGO_TARGET_DIR/debug/examples/normalization_probe" > "$run_dir/normalization.json"
cargo build --locked --offline -p tf-exec --example polars_probe
"$python_bin" -m pip wheel --no-index --no-deps --no-build-isolation --wheel-dir "$run_dir/wheels" ./python
"$python_bin" -I -B python/tools/check_polars_execution.py "$CARGO_TARGET_DIR/debug/examples/polars_probe" "$CARGO_TARGET_DIR/debug/examples/normalization_probe" "$run_dir/wheels/transflow-0.0.0.dev0-py3-none-any.whl" > "$run_dir/polars-execution.json"
cargo build --locked --offline -p tf-exec --example checks_probe
"$python_bin" -I -B python/tools/check_expectations.py "$CARGO_TARGET_DIR/debug/examples/checks_probe" "$CARGO_TARGET_DIR/debug/examples/polars_probe" "$run_dir/wheels/transflow-0.0.0.dev0-py3-none-any.whl" > "$run_dir/expectation-evaluation.json"
cargo build --locked --offline -p tf-exec --example lifecycle_probe
"$python_bin" -I -B python/tools/check_lifecycle.py "$CARGO_TARGET_DIR/debug/examples/lifecycle_probe" "$run_dir/wheels/transflow-0.0.0.dev0-py3-none-any.whl" > "$run_dir/expectation-lifecycle.json"
cargo build --locked --offline -p transflow --bin transflow --example dispatch_probe
"$python_bin" -I -B python/tools/check_dispatch.py "$CARGO_TARGET_DIR/debug/examples/dispatch_probe" "$CARGO_TARGET_DIR/debug/transflow" "$run_dir/wheels/transflow-0.0.0.dev0-py3-none-any.whl" "$repo_root/target/qualification/wheelhouse" > "$run_dir/build-dispatch.json"
"$python_bin" -I -B python/tools/check_developer_core.py "$CARGO_TARGET_DIR/debug/transflow" "$run_dir/wheels/transflow-0.0.0.dev0-py3-none-any.whl" "$repo_root/target/qualification/wheelhouse" --output "$run_dir/developer-core.json"
"$python_bin" -I -B python/tools/check_foreign.py "$CARGO_TARGET_DIR/debug/transflow" "$run_dir/wheels/transflow-0.0.0.dev0-py3-none-any.whl" "$repo_root/target/qualification/wheelhouse" --output "$run_dir/foreign-data.json"
node tools/qualification/web/patch-elk.mjs
npm --prefix tools/qualification/web run typecheck
npm --prefix tools/qualification/web test
npm --prefix tools/qualification/web run build
echo "Dependency qualification passed. Native reports: $run_dir"
echo "This includes build dispatch, public CLI, restart and customer-orders journeys; release conformance remains a later task."
