#!/usr/bin/env bash
# Requires previously fetched locked dependencies; does not install anything.
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd -- "$repo_root"
python_bin="${PYTHON:-python}"
"$python_bin" -B tools/check_rust.py
"$python_bin" -B -m unittest discover -s tools/tests -v
cargo fmt --all -- --check
cargo check --workspace --locked --offline
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo test --workspace --locked --offline
