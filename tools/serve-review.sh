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

# Prefer the repository's already-prepared pinned Node; never install on launch.
review_node_platform=""
case "$(uname -s):$(uname -m)" in
    Darwin:arm64) review_node_platform="darwin-arm64" ;;
    Darwin:x86_64) review_node_platform="darwin-x64" ;;
    Linux:aarch64|Linux:arm64) review_node_platform="linux-arm64" ;;
    Linux:x86_64) review_node_platform="linux-x64" ;;
esac
review_node_version="$(< .node-version)"
review_node_bin="$review_root/target/toolchains/node-v$review_node_version-$review_node_platform/bin"
if [[ -n "$review_node_platform" && -x "$review_node_bin/node" && -x "$review_node_bin/npm" ]]; then
    export PATH="$review_node_bin:$PATH"
fi

cargo build --locked --offline -p transflow --target-dir "$review_root/target"
npm --prefix web run prepare:types
npm --prefix web run build
printf 'Starting review with the rebuilt CLI and web/dist from %s\n' "$review_root"
exec "$review_root/target/debug/transflow" --workspace "$review_workspace" serve \
    --ui-dir "$review_root/web/dist" "$@"
