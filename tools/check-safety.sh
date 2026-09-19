#!/usr/bin/env bash
# Standard-library checks; this entry point never fetches dependencies/advisories.
set -euo pipefail
implementation_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd -- "$implementation_root"
python_bin="${PYTHON:-python}"
"$python_bin" -B -m unittest discover -s tools/safety/tests -v
"$python_bin" -B tools/safety/check.py "$@"
