#!/usr/bin/env bash
# Requires previously fetched locked dependencies; does not install anything.
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd -- "$repo_root"
python_bin="${PYTHON:-python}"
"$python_bin" -B tools/check_rust.py
"$python_bin" -B -m unittest discover -s tools/tests -v
# CI summaries contain fixed phase labels and allowlisted public function names only.
run_rust_check() {
    local stage="$1"
    shift
    if "$@"; then
        return 0
    fi
    if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
        printf 'Rust check failed during: %s
' "$stage" >> "$GITHUB_STEP_SUMMARY"
    fi
    printf '::error title=Rust check failure::Failed stage: %s\n' "$stage"
    return 1
}
run_rust_check formatting cargo fmt --all -- --check
run_rust_check compilation cargo check --workspace --locked --offline
# The native engine probe builds tf-store alone; do not inherit other crates' Serde features.
run_rust_check standalone-storage cargo check -p tf-store --example normalization_probe --locked --offline
run_rust_check linting cargo clippy --workspace --all-targets --locked --offline -- -D warnings
mkdir -p target
if ! cargo test --workspace --locked --offline 2>&1 | tee target/rust-tests.log; then
    if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
        public_failure_names="$("$python_bin" tools/rust_failure_summary.py target/rust-tests.log)"
        printf '%s\n' "$public_failure_names" >> "$GITHUB_STEP_SUMMARY"
        printf '::error title=Rust test failure::%s\n' "$public_failure_names"
    fi
    exit 1
fi
