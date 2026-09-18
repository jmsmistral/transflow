# Contributor verification

The T001–T002 foundation uses Python's standard library and Git. It was verified
on macOS 15.3.1 arm64 with Python 3.14.7 from the project's `transflow` pyenv
environment. Python 3.11+ is the checker syntax baseline; other interpreters and
Linux have not yet been run. Application toolchain qualification belongs to T003.

## Run the current checks

```bash
cd ~/dev/transflow
bash tools/check.sh
```

The script resolves both checkout paths relative to itself, enters the
implementation root so pyenv selects `.python-version`, and stops on failure.
An optional `PYTHON` environment variable can name another interpreter executable.
It requires the sibling specification checkout for contributor verification.

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

Only the documentation checker and its regression tests currently run. Add the
Rust checks in T004, Python checks in T005, frontend checks in T006, and CI,
dependency/license/privacy checks in T008 as their real manifests and runners
become available. Add schema drift, canonical fixtures, integration/recovery and
browser gates with their implementations. A missing required runner must fail,
not silently count as a pass. Explicit dependency setup remains separate.

No application correctness, installed-runtime independence, release archive
contents, engine compatibility or full acceptance scenario is established by
this script. The [task evidence](../../../transflow-spec/TASKS.md) and
[current verification report](../../../transflow-spec/VERIFICATION.md) record
what was actually run. Review changes in both repositories before committing;
record each revision separately when those commits exist.
