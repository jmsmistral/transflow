#!/usr/bin/env bash
# Cross-language lifecycle tests require both explicitly prepared toolchains.
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd -- "$repo_root"
python_bin="${PYTHON_CHECK:-$repo_root/target/python/py314/bin/python}"
export PIP_NO_INDEX=1 PIP_DISABLE_PIP_VERSION_CHECK=1 PYTHONDONTWRITEBYTECODE=1
cargo build --locked --offline -p transflow --bins --example planning_probe --example inspection_fixture
"$python_bin" -m pytest -c python/pyproject.toml python/integration --junitxml=target/cli-tests.xml
