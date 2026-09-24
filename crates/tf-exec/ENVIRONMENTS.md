# Explicit environment preparation (T025)

The CLI now supports `env lock`, `env sync` and read-only `env check`. Each uses an
explicit workspace and borrowed coordinator ownership. `--python` wins over the
local interpreter locator; otherwise the configured `python3.14` is
located on PATH. Only lock/sync require the qualified pip 26.2.1 and pip-tools
7.6.1 installed in that tooling interpreter. Nothing installs tooling implicitly.

The Rust binary embeds the standard-library environment service, so missing SDK
metadata cannot cause an unrelated package with the same index name to be installed.
Lock resolves ordinary index requirements to exact versions/SHA-256 hashes, using
wheels only. Its header records the interpreter/ABI/platform target, resolver pins
and dependency-input digest. Resolution uses captured input bytes and local
runtime caches. Unknown flags, editable/direct-URL requirements and nested input
files are rejected in this initial contract. The `transflow` distribution must not
appear in dependency inputs or locks; it is supplied separately.

T068 locks the qualified DuckDB 1.5.5 check engine alongside user requirements,
including custom inputs that do not mention it. The captured resolver input adds
the managed pin; the authored requirements file is unchanged. A conflicting user
pin fails resolution and preserves the previous lock. Sync/check reject older
locks missing this engine and request explicit lock/sync. No build-time install
or format/version change is introduced.

Sync requires `--runtime-wheel PATH` while no release bundle is published. It
validates the wheel's distribution/version and presence of both modules, captures
its bytes, installs locked dependencies and that wheel explicitly via pip, checks
package consistency and SDK/worker compatibility, and inventories actual installed
files. There is no index fallback for the Transflow wheel. `--wheelhouse DIRECTORY`
and `--no-index` support fully offline setup with prepared artifacts.

Environments live under `runtime/environments/<specification-fingerprint>/<generation>`.
A venv contains absolute paths, so it is created at its final generation path and
never relocated. The specification fingerprint covers target, lock, runtime wheel
and base interpreter; actual installed-state fingerprints also cover package files,
entrypoints, venv configuration and interpreter bytes. They may differ across
fresh generations because installed scripts/config contain generation paths.
A ready pointer changes only after success; failed sync removes its new generation
and preserves the previous environment. Old generations are retained for later GC.

`env check` checks the lock target/input, selected interpreter and actual installed
state without running pip or importing packages. It detects altered/added/missing
files, missing SDK metadata, interpreter/config drift and mutable editable installs.
It never executes installed `.pth` files during inspection. A mismatch requires an
explicit sync; ordinary builds use this same check before execution.

The initial lock is target-specific: changing Python patch/ABI/platform requires an
explicit lock for that target. Package outputs are not exposed as raw diagnostic
logs. Each package command has a ten-minute deadline; the explicit environment
operation has a thirty-minute outer limit. This is separate from transform timers.
