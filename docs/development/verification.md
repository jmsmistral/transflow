# Contributor verification

The contributor checks use Python's standard library and Git; T004 adds the
pinned Rust toolchain, rustfmt and Clippy; T005 adds hash-locked Python package
tooling in a separate repository-local environment. Document checks were verified
on macOS 15.3.1 arm64 with Python 3.14.7 from the project's `transflow` pyenv
environment. Python 3.11+ is the checker syntax baseline; other interpreters and
Linux have not yet been run for the document checker. The separate
[T003 compatibility matrix](compatibility.md) records dependency probe results.

## Run the current checks

```bash
cd ~/dev/transflow
cargo fetch --locked  # Explicit first-time setup; requires registry access.
python -m venv target/python/py314
target/python/py314/bin/python -m pip install --require-hashes --only-binary=:all: -r python/dev-py314.lock
bash tools/check.sh
```

The script resolves both checkout paths relative to itself, enters the
implementation root so pyenv selects `.python-version`, and stops on failure.
An optional `PYTHON` environment variable can name another interpreter executable.
It requires the sibling specification checkout for contributor verification.
After explicit setup, checks use locked, offline Cargo resolution. The Rust-only
entry point, `bash tools/check-rust.sh`, works without the sibling specification.
Python package checks also work independently through `bash tools/check-python.sh`;
`PYTHON_CHECK` can select an absolute tooling interpreter path, defaulting to
`target/python/py314/bin/python`. See [Python setup](../../python/README.md).

The checker inventories each repository using Git, checks all tracked and
nonignored candidate Markdown, and checks Python/TOML/JSON fences by parsing them.
It validates local inline links, reference definitions and ATX heading anchors;
remote links are counted without network access. It never runs fenced examples.
The mutation tests use disposable paired repositories and synthetic files.

To get machine-readable documentation results or audit a separate unpacked
public export, invoke the canonical checker directly:

```bash
python -B ../transflow-spec/tools/check_spec.py --json
python -B ../transflow-spec/tools/check_spec.py --export-root /tmp/transflow-public-export
```

The export directory must already exist. Exports are audited separately, without
Git ignore rules. Repository inventory excludes ignored private references;
tracked private paths fail even when their working files have been deleted.
Both checks reject the private reference directory, mapped screenshot basenames,
Finder metadata and symlinks that cannot be safely audited. Original public image
assets are allowed. These are path hygiene checks, not image-content recognition
or a secret scanner: renamed copies and arbitrary embedded private content still
need review and the T008/T120 checks. No private image bytes are loaded.

## Extending the aggregate contract

The contributor aggregate runs document validation and 43 tooling regression
tests, followed by ten Rust dependency-checker tests and the following gates:

```bash
cargo fmt --all -- --check
cargo check --workspace --locked --offline
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo test --workspace --locked --offline
```

The dependency checker examines declared normal, optional, target, build and dev
dependencies by package identity, rejects forbidden edges/cycles, requires exact
pins and verifies external lock entries against T003's probe lockfile. It also
checks that each crate inherits workspace lints. Expanding its allowlists requires
explicit architectural and qualification review.

Five Rust tests cover the CLI: help/version and rejected commands in a subprocess
outside the checkouts with an empty PATH, plus typed stdout/stderr failures.
The nine other crates contain no runtime implementation or tests yet. The
[Rust CI workflow](../../.github/workflows/rust.yml) runs the same Rust entry point
on macOS arm64 and Linux x86_64/arm64; it does not need the specification checkout.
Its remote results are pending.

The aggregate then invokes Python tooling lock verification, `pip check`, Ruff
lint/format, strict mypy and pytest. Package tests build one `transflow` wheel from
explicit source roots copied outside both checkouts. They verify that the wheel
contains both typed modules, has one version/distribution identity and needs no
engine imports or workspace activity for SDK import. Actual pip installation in
a clean environment is offline. Downstream mypy checks use installed wheel types;
invalid argument types fail. Worker tests cover both entry points, unavailable
operations, protocol mismatch, distribution drift and failed output streams.

The [Python CI workflow](../../.github/workflows/python.yml) covers both Python
versions on all three target platforms and preserves wheel hashes and JUnit
reports. Native local execution uses Python 3.14.7 and the available 3.13.0;
CI's Python 3.13.15 and Linux results remain pending. Wire protocol `0.0` is only
bootstrap metadata; no framed transport or execution operation is implemented.

Run `bash tools/qualification/check.sh` separately with its prepared Python
environment for T003 native dependency probes; the compatibility guide documents
explicit setup. Checks never fetch external dependencies; Python tests install
the locally built wheel into disposable environments. Add frontend checks in T006 and broader CI,
dependency/license/privacy checks in T008 as their real manifests and runners
become available. Add schema drift, canonical fixtures, integration/recovery and
browser gates with their implementations. A missing required runner must fail,
not silently count as a pass. Explicit dependency setup remains separate.

The executable tests establish that help/version need no specification checkout,
Python or Git; installed Python bootstrap tests need neither repository. No pipeline correctness, release archive
contents, engine compatibility or full acceptance scenario is established by
this script. The [task evidence](../../../transflow-spec/TASKS.md) and
[current verification report](../../../transflow-spec/VERIFICATION.md) record
what was actually run. Review changes in both repositories before committing;
record each revision separately when those commits exist.
