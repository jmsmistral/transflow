#!/usr/bin/env bash
# Contributor, Rust, Python and web checks; dependency setup is explicit.
set -euo pipefail

implementation_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
spec_root="$(cd -- "$implementation_root/.." && pwd -P)/transflow-spec"
python_bin="${PYTHON:-python}"

if [[ ! -f "$spec_root/tools/check_spec.py" ]]; then
    echo "Missing specification checker: $spec_root/tools/check_spec.py" >&2
    echo "Check out the matching transflow-spec repository beside transflow." >&2
    exit 1
fi

# Resolve pyenv's project-local interpreter even when invoked from another cwd.
cd -- "$implementation_root"
"$python_bin" -B "$spec_root/tools/check_spec.py" \
    --root "$spec_root" --implementation-root "$implementation_root"
"$python_bin" -B -m unittest discover -s "$spec_root/tools/tests" -v
bash "$implementation_root/tools/check-safety.sh"
bash "$implementation_root/tools/check-rust.sh"
bash "$implementation_root/tools/check-python.sh"
bash "$implementation_root/tools/check-web.sh"
bash "$implementation_root/tools/check-cli.sh"
echo "Contributor, safety, Rust, Python and web bootstrap checks passed. Release conformance remains a later task."
