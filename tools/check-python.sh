#!/usr/bin/env bash
# Requires explicit tooling setup. Builds and installs only local test wheels.
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd -- "$repo_root"
python_bin="${PYTHON_CHECK:-$repo_root/target/python/py314/bin/python}"
if [[ ! -x "$python_bin" ]]; then
    echo "Missing Python tooling environment: $python_bin" >&2
    echo "Follow python/README.md to prepare it, or set PYTHON_CHECK to its absolute interpreter path." >&2
    exit 1
fi
export PIP_NO_INDEX=1 PIP_DISABLE_PIP_VERSION_CHECK=1 PYTHONDONTWRITEBYTECODE=1
"$python_bin" -B python/tools/check_environment.py
"$python_bin" -m pip check
"$python_bin" -m ruff check --config python/pyproject.toml python tools/safety
"$python_bin" -m ruff format --config python/pyproject.toml --check python tools/safety
"$python_bin" -m mypy --config-file python/pyproject.toml
python_minor="$("$python_bin" -c 'import sys; print(f"py{sys.version_info.major}{sys.version_info.minor}")')"
mkdir -p "target/python/$python_minor"
"$python_bin" -m pytest -c python/pyproject.toml python/tests \
    --junitxml="target/python/$python_minor/tests.xml"
