# Dependency and privacy baseline

T008 adds offline contributor checks and a separate CI audit. It does not qualify
application authentication, SQL isolation, release archives or full A38/A40
acceptance. The four existing language/native workflows retain their supported
platform matrices. The [safety workflow](../../.github/workflows/safety.yml) checks
repository metadata on Ubuntu 24.04; that is not another native qualification.

## Run and refresh

```bash
cd ~/dev/transflow
bash tools/check-safety.sh
bash tools/check.sh
```

Both commands use already prepared dependencies and make no network requests.
The safety runner itself requires only Python 3.11+ and Git. Its default local mode
also requires the sibling `~/dev/transflow-spec` checkout. It runs synthetic failure tests, scans both
repositories, checks inventory/advisory/exception consistency and verifies
registered generated contracts. CI uses the standalone mode:

```bash
bash tools/check-safety.sh --implementation-only
```

This scans only `transflow` and never reads the specification checkout or its
screenshot-name mapping. Credential patterns, the private reference-directory
rule, symlink/size guards, dependency/license/advisory policy and generated-contract
checks still run. Default local checks continue to require the sibling checkout;
there is no automatic fallback that hides a missing local specification.

The Python language gate also lints/formats the
safety tooling with the repository's pinned Ruff configuration.

Refresh advisory evidence explicitly when it is older than seven UTC calendar
days or the inventory changes:

```bash
python -B tools/safety/advisories.py --output security/advisories.json
bash tools/check-safety.sh
```

This sends only public package names, ecosystems and exact versions to the
[OSV batch API](https://google.github.io/osv.dev/post-v1-querybatch/). It does not
send source, dependency paths, credentials or reference images. Queries are
batched, response size/time/page counts are bounded, and pagination is followed.
Network failures, incomplete responses and pagination loops fail. The refresh
command collects evidence; the subsequent safety check applies policy and fails
on unexcepted findings. A completed query with no findings means no advisories
were reported for those versions at that time, not that they are vulnerability-free.

After an intentional lockfile change, first regenerate the license inventory:

```bash
cargo fetch --locked
cargo fetch --locked --manifest-path tools/qualification/rust/Cargo.toml
python -B tools/safety/refresh-inventory.py
python -B tools/safety/advisories.py --output security/advisories.json
bash tools/check-safety.sh
```

Cargo fetch prepares every locked target's metadata; it does not change versions.
Inventory refresh uses offline Cargo metadata, npm lockfile declarations and
public PyPI release metadata. If a Python release lacks license metadata, it reads
a license file from a wheel whose SHA-256 is already in the locks, without
installing it. No new package manager or audit dependency is installed.
Review the resulting inventory and exception changes together with the locks.

## Inventory and exceptions

[dependencies.json](../../security/dependencies.json) records 634 distinct package
versions: 216 crates.io, 391 npm and 27 PyPI. Every entry includes its contributing
lockfiles and declared license text/expression. The nine locks cover application,
package/web tooling and isolated T003 qualification dependencies, including
platform-specific transitive packages. They are not all shipped runtime dependencies.
Lock SHA-256 values bind the inventory; the advisory report is bound to the full
inventory digest and exact queried package list. New/unrecognized lockfiles,
unqualified registry sources, missing checksums and missing licenses fail.

These are upstream declarations, not a legal approval or release notice bundle.
Some older PyPI packages supply broad license classifiers or full license text
instead of SPDX expressions. Maintainers must review changes; T116/T120 still
need shipped-artifact license/notice and privacy verification. First-party source
licensing is a separate release decision.

[policy.json](../../security/policy.json) permits only exact finding exceptions
with an owner, rationale and expiry. An exception expires at the start of its
stated UTC date. Wildcards, duplicate/unused exceptions and missing ownership or
rationale fail. All reported advisories block by default, without a severity
threshold; secret findings cannot be suppressed through this policy.

Two development-only deprecations are temporarily excepted until 2026-10-19:

| Package | Reason to review the related toolchain together |
|---|---|
| ESLint 9.39.5 | The qualified jsx-a11y plugin's peer range ends at ESLint 9. |
| whatwg-encoding 3.1.1 | Transitive jsdom dependency; newer jsdom needs a later Node patch than the qualified 24.4.1 baseline. |

The Transflow maintainers own both reviews. Neither exception suppresses a
vulnerability advisory. No dependency version was changed for T008.

## Privacy scope

The scanner uses each explicit repository's tracked and nonignored candidate
files, including fixtures and examples. It never traverses `~/dev/`. Canonical
screenshot basenames come from the specification's mapping; private reference
paths are rejected before reading bytes in local paired checks. CI does not
load the private mapping; its path checks cover the reference directory and
Finder metadata, while credential scanning remains enabled. Tracked private paths fail even if their
working files were deleted. Ignored reference images are neither read nor hashed.
Symlinks and files larger than 16 MiB fail instead of silently passing.

The scanner recognizes GitHub tokens, AWS access key identifiers, private-key
headers and selected long literal credential assignments. Findings show path,
line, rule and a short fingerprint, never the matching value or source line.
Synthetic tests construct fake credentials at runtime so no real credentials are
needed. If a real credential is found, rotate/revoke it through the appropriate
explicit owner action, remove it from public files and review history separately.

This is a stable-working-tree check, not a hostile-filesystem sandbox or a proof
of privacy. It does not inspect Git history, ignored runtime files, decoded
archives, image content, renamed screenshot copies or arbitrary private names
and datasets. Encoded/unknown credential formats can escape these patterns.
Human review and the later release privacy audit remain necessary. The canonical
document checker also retains its separate path/export checks.

## Generated contracts

[generated-contracts.json](../../security/generated-contracts.json) is explicitly
empty: the scaffold has no generated domain/API contracts yet. The runner reports
that state, rather than claiming generated-output conformance. T012 adds one
[authored schema and cross-language fixtures](../../schemas/README.md), without
generated copies. T014/T074 must register generators as they introduce outputs. Artifacts under a `generated/`
directory or with `.generated.` in their filename must be registered; other
naming conventions require an explicit discovery-rule update.

A registry entry contains `name`, an `inputs` object mapping relative source and
generator paths to SHA-256 values, an `outputs` list of relative paths, and a
`command` array beginning with `{python}` followed by a repository generator
script and arguments. Inputs are copied into a temporary directory; the generator
runs there through the prepared Python interpreter with a 60-second timeout.
Outputs must match the committed files byte for byte. Changed input digests,
unregistered outputs, failed generators and stale bytes fail. The source checkout
is not rewritten. Generators are trusted reviewed code, not sandboxed code;
future Rust-backed generators need an explicit offline dependency setup contract.
Tests exercise an actual synthetic generator and verify that drift is detected.

## CI and publishing boundary

All five workflows have `contents: read`, full commit action pins, checkout
credential persistence disabled, bounded jobs, and push/PR/manual triggers.
There are no publishing jobs, registry credentials, secret references,
`pull_request_target` triggers or interpolated PR text in shell commands. These
choices follow GitHub's [secure-use guidance](https://docs.github.com/en/actions/reference/security/secure-use).
The existing build jobs install only committed locked dependencies.

Safety CI queries current advisories into `target/safety/advisories.json`, applies
the same offline policy checks, and retains that public dependency evidence for
14 days. A failed query fails the job, even if the older committed report exists.
No source dumps or reference assets are uploaded by this workflow.

CI checks out only `transflow`. At the owner's request, the private specification
repository remains separate implementation guidance and knowledge: CI does not
fetch it, run its document checker/regressions or require cross-repository
credentials. Local aggregate checks still use the current sibling working tree.
Local verification and remote CI results are recorded separately in the
[task ledger](../../../transflow-spec/TASKS.md).
Publishing and credential/account changes remain separate explicit owner actions.
