# Transflow Python distribution

One distribution named **transflow** contains two typed Python modules:
`transflow` (SDK) and `transflow_worker` (worker). They share one version and wheel,
with source responsibilities retained under `sdk/` and `worker/`.

The development package supports Python 3.13 and 3.14. It provides version and
bootstrap compatibility metadata, plus worker help/version/compatibility commands.
Transform declarations, catalogue references, engine adapters and execution are
not implemented yet. Importing the SDK does not initialize a workspace or worker.

## Contributor setup and checks

From the implementation root, prepare a local tooling environment using the
project's pyenv-selected Python 3.14 interpreter:

```bash
python -m venv target/python/py314
target/python/py314/bin/python -m pip install --require-hashes --only-binary=:all: -r python/dev-py314.lock
bash tools/check-python.sh
```

For Python 3.13, create `target/python/py313` with that interpreter, install
`python/dev-py313.lock`, and run:

```bash
PYTHON_CHECK="$PWD/target/python/py313/bin/python" bash tools/check-python.sh
```

The runner verifies tooling pins, Ruff, strict mypy, unit tests and actual wheel
builds/isolated installations. It builds with the pinned setuptools backend using
`--no-isolation`; pip installs only local wheels with index access disabled.
Neither checkout is needed by the installed package. Tests retain a wheel and its
SHA-256 under `target/python/py314/wheels/` (or `py313`) and a JUnit report beside it.

After a successful check, try the single wheel in a disposable environment:

```bash
python -m venv target/python/demo
target/python/demo/bin/python -m pip install --no-index target/python/py314/wheels/transflow-0.0.0.dev0-py3-none-any.whl
target/python/demo/bin/python -I -m transflow_worker compatibility
```

The report checks installed package/version consistency and protocol `0.0`, a
provisional bootstrap identifier with no supported execution operations. Explicit
unknown major/minor versions fail. T012 will define the framed wire schemas and
negotiation; this diagnostic JSON is not the worker control channel.

## Distribution and tooling decisions

The owner selected `transflow` as the single PyPI distribution name. The PyPI JSON
lookup returned HTTP 404 on 2026-09-19; this neither reserves the name nor proves
publication eligibility. Nothing has been published. The future public install
command is `pip install transflow`; use the local wheel until a release exists.
The Rust executable is still a separate native artifact.

The build backend is setuptools 84.0.0 with build 1.6.1, already present in T003's
resolver baseline. Development pins add Ruff 0.16.8, mypy 2.3.1 and pytest 9.1.1.
The two interpreter-specific locks include transitive hashes. Engines are not
dependencies of this bootstrap wheel; add the qualified runtime dependencies as
their implementations land. Normal checks do not resolve or download packages.

To deliberately regenerate locks, use the corresponding prepared interpreter's
pip-tools 7.6.1 with `python/requirements-dev.in`, `--generate-hashes`,
`--allow-unsafe`, `--strip-extras`, `--no-header` and `--no-emit-index-url`.
Review changes and requalify both interpreters and target platforms.
