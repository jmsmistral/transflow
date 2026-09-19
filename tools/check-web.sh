#!/usr/bin/env bash
# Offline web checks after explicit npm ci; browser journeys use the internal Browser.
set -euo pipefail

implementation_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd -- "$implementation_root/web"
node --input-type=module -e '
import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
const { engines } = JSON.parse(readFileSync("package.json", "utf8"));
const npm = execFileSync("npm", ["--version"], { encoding: "utf8" }).trim();
if (process.versions.node !== engines.node || npm !== engines.npm) {
  throw new Error(`Use Node ${engines.node} and npm ${engines.npm}; found ${process.versions.node}/${npm}.`);
}
'
if [[ ! -d node_modules ]]; then
    echo "Prepare web dependencies first: npm --prefix web ci --ignore-scripts" >&2
    exit 1
fi
mkdir -p -- "$implementation_root/target/web"
npm run typecheck
npm run lint
npm run format:check
npm run test -- --reporter=default --reporter=json --outputFile=../target/web/unit-tests.json
npm run build
npm run build:verify
echo "Web component and production build checks passed. Browser journeys are a separate internal Browser check."
