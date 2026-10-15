#!/usr/bin/env bash
# Rebuild this checkout's contributor CLI and UI before starting review.
# Dependencies and the workspace environment must already be prepared explicitly.
set -euo pipefail

if [[ $# -eq 0 || "$1" == "--help" || "$1" == "-h" ]]; then
    printf 'Usage: bash tools/serve-review.sh WORKSPACE [serve options, e.g. --open]\n'
    if [[ $# -eq 0 ]]; then exit 2; fi
    exit 0
fi

# Resolve a relative workspace before moving into the implementation checkout.
if [[ ! -d "$1" ]]; then
    printf 'Workspace directory does not exist: %s\n' "$1" >&2
    exit 2
fi
review_workspace="$(cd -- "$1" && pwd -P)"
shift
review_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd -- "$review_root"

if [[ ! -d web/node_modules ]]; then
    printf 'Prepare web dependencies first: npm --prefix web ci --ignore-scripts\n' >&2
    exit 1
fi

cargo build --locked --offline -p transflow --target-dir "$review_root/target"
npm --prefix web run prepare:types
npm --prefix web run build
printf 'Starting review with the rebuilt CLI and web/dist from %s\n' "$review_root"
exec "$review_root/target/debug/transflow" --workspace "$review_workspace" serve \
    --ui-dir "$review_root/web/dist" "$@"
