"""Build actual wheels and test pip installs without a checkout or network."""

import hashlib
import json
import os
import shutil
import subprocess
import sys
import venv
from email.parser import Parser
from pathlib import Path
from zipfile import ZipFile

import pytest

PYTHON_ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = PYTHON_ROOT.parent
VERSION = "0.0.0.dev0"


def run(command: list[str], cwd: Path, *, ok: bool = True) -> subprocess.CompletedProcess[str]:
    environment = dict(os.environ)
    environment.pop("MYPYPATH", None)
    environment.pop("PYTHONPATH", None)
    environment.update(
        PIP_NO_INDEX="1", PIP_DISABLE_PIP_VERSION_CHECK="1", PYTHONDONTWRITEBYTECODE="1"
    )
    result = subprocess.run(
        command, cwd=cwd, env=environment, text=True, capture_output=True, timeout=90, check=False
    )
    if ok:
        assert result.returncode == 0, result.stdout + result.stderr
    return result


def pip(
    python: Path, args: list[str], cwd: Path, *, ok: bool = True
) -> subprocess.CompletedProcess[str]:
    # Use the hash-locked contributor pip; clean runtimes need no pip installation.
    return run([sys.executable, "-m", "pip", "--python", str(python), *args], cwd, ok=ok)


def clean_python(root: Path) -> Path:
    venv.EnvBuilder(with_pip=False).create(root)
    return root / "bin/python"


@pytest.fixture(scope="session")
def wheel(tmp_path_factory: pytest.TempPathFactory) -> Path:
    root = tmp_path_factory.mktemp("wheel-build")
    output = root / "wheels"
    source = root / "source"
    source.mkdir()
    # Build only the explicit packaging inputs, outside either repository.
    for name in ("pyproject.toml", "README.md"):
        shutil.copy2(PYTHON_ROOT / name, source / name)
    for name in ("sdk", "worker"):
        shutil.copytree(
            PYTHON_ROOT / name,
            source / name,
            ignore=shutil.ignore_patterns("build", "dist", "*.egg-info", "__pycache__"),
        )
    run(
        [
            sys.executable,
            "-m",
            "build",
            "--wheel",
            "--no-isolation",
            "--outdir",
            str(output),
            str(source),
        ],
        root,
    )
    wheel = next(output.glob(f"transflow-{VERSION}-*.whl"))
    artifacts = (
        REPOSITORY
        / "target/python"
        / f"py{sys.version_info.major}{sys.version_info.minor}"
        / "wheels"
    )
    artifacts.mkdir(parents=True, exist_ok=True)
    shutil.copy2(wheel, artifacts / wheel.name)
    digest = hashlib.sha256(wheel.read_bytes()).hexdigest()
    (artifacts / "sha256.json").write_text(json.dumps({wheel.name: digest}, indent=2) + "\n")
    return wheel


@pytest.fixture(scope="session")
def installed(wheel: Path, tmp_path_factory: pytest.TempPathFactory) -> Path:
    root = tmp_path_factory.mktemp("installed-transflow")
    python = clean_python(root / "env")
    pip(python, ["install", "--no-index", "--only-binary=:all:", str(wheel)], root)
    pip(python, ["check"], root)
    return python


def test_one_wheel_contains_both_typed_modules(wheel: Path) -> None:
    modules = {"__init__.py", "_version.py", "compatibility.py", "py.typed"}
    expected = {f"transflow/{module}" for module in modules}
    expected |= {f"transflow_worker/{module}" for module in modules | {"__main__.py", "cli.py"}}
    expected |= {
        f"transflow/{name}.py"
        for name in (
            "_catalog_prototype",
            "catalog",
            "testing",
            "declarations",
            "_declaration_values",
            "expectations",
        )
    }
    expected |= {
        f"transflow_worker/{name}.py"
        for name in (
            "wire",
            "canonical",
            "_wire_assertions",
            "_wire_schema",
            "_wire_validators",
            "source_index",
            "environment",
        )
    }
    metadata_files = {"METADATA", "WHEEL", "RECORD", "top_level.txt", "entry_points.txt"}
    expected |= {f"transflow-{VERSION}.dist-info/{name}" for name in metadata_files}
    with ZipFile(wheel) as archive:
        assert set(archive.namelist()) == expected
        metadata = Parser().parsestr(
            archive.read(f"transflow-{VERSION}.dist-info/METADATA").decode()
        )
        assert metadata["Name"] == "transflow"
        assert metadata["Version"] == VERSION
        assert metadata["Requires-Python"] == "<3.15,>=3.13"
        assert metadata.get_all("Requires-Dist", []) == []


def test_sdk_imports_without_runtime_activity(installed: Path, tmp_path: Path) -> None:
    # Normal imports may read code, but must not read data, spawn or connect.
    code = """
import sys
def audit(event, args):
    if event.startswith(("subprocess.", "socket.", "sqlite3.")) or event == "os.system":
        raise RuntimeError("Unexpected SDK activity: " + event)
    if event == "open":
        path = str(args[0])
        if not path.endswith((".py", ".pyc")):
            raise RuntimeError("Unexpected SDK file read: " + path)
sys.addaudithook(audit)
import transflow
import transflow.catalog
import transflow.testing
assert dir(transflow.catalog.C) == []
assert transflow.__version__ == "0.0.0.dev0"
assert not ({"transflow_worker", "polars", "pandas", "duckdb", "pyarrow"} & sys.modules.keys())
print(transflow.__version__)
"""
    result = run([str(installed), "-I", "-B", "-c", code], tmp_path)
    assert result.stdout.strip() == VERSION
    assert result.stderr == ""
    assert not list(tmp_path.iterdir())


def test_isolated_worker_ignores_source_shadowing(installed: Path, tmp_path: Path) -> None:
    (tmp_path / "transflow.py").write_text('raise RuntimeError("ambient module imported")\n')
    (tmp_path / "transflow_worker.py").write_text('raise RuntimeError("ambient worker imported")\n')
    result = run([str(installed), "-I", "-m", "transflow_worker", "compatibility"], tmp_path)
    assert json.loads(result.stdout) == {
        "distribution_version": VERSION,
        "protocol_major": 1,
        "protocol_minor": 0,
        "wire_protocol_implemented": True,
        "supported_operations": [],
    }
    assert result.stderr == ""
    assert sorted(path.name for path in tmp_path.iterdir()) == [
        "transflow.py",
        "transflow_worker.py",
    ]


@pytest.mark.parametrize("args", [["--help"], ["--version"], []])
def test_worker_entrypoints(installed: Path, tmp_path: Path, args: list[str]) -> None:
    module = run([str(installed), "-I", "-m", "transflow_worker", *args], tmp_path)
    console = run([str(installed.parent / "transflow-worker"), *args], tmp_path)
    assert module.stdout == console.stdout
    assert module.stderr == console.stderr == ""
    assert VERSION in module.stdout if args == ["--version"] else "not implemented" in module.stdout


@pytest.mark.parametrize(
    "args",
    [
        ["execute"],
        ["discover"],
        ["--unknown"],
        ["compatibility", "--protocol-major", "2"],
        ["compatibility", "--protocol-minor", "4294967296"],
        ["compatibility", "--protocol-major", "-1"],
    ],
)
def test_worker_rejects_unavailable_operations_and_protocols(
    installed: Path, tmp_path: Path, args: list[str]
) -> None:
    result = run([str(installed), "-I", "-m", "transflow_worker", *args], tmp_path, ok=False)
    assert result.returncode != 0
    assert result.stdout == ""
    assert result.stderr
    assert "Traceback" not in result.stderr
    assert not list(tmp_path.iterdir())


def test_single_distribution_owns_both_modules(installed: Path, tmp_path: Path) -> None:
    code = """
from importlib.metadata import packages_distributions, version
import transflow
import transflow_worker
owners = packages_distributions()
assert owners['transflow'] == ['transflow']
assert owners['transflow_worker'] == ['transflow']
assert transflow.__version__ == transflow_worker.__version__ == version('transflow')
"""
    run([str(installed), "-I", "-c", code], tmp_path)


def test_installed_sdk_exposes_types(installed: Path, tmp_path: Path) -> None:
    valid = tmp_path / "consumer.py"
    valid.write_text(
        "from transflow import ProtocolVersion, require_protocol\n"
        "require_protocol(ProtocolVersion(1, 0))\n"
        "from transflow import Input, Output, transform\n"
        "@transform(value=Input('raw/value'), output=Output('out'))\n"
        "def ordinary(value: int) -> int: return value + 1\n"
        "answer: int = ordinary(1)\n"
    )
    command = [
        sys.executable,
        "-m",
        "mypy",
        "--strict",
        "--python-executable",
        str(installed),
        str(valid),
    ]
    run(command, tmp_path)
    valid.write_text(valid.read_text() + 'ordinary("wrong")\nProtocolVersion("wrong", 0)\n')
    result = run(command, tmp_path, ok=False)
    assert result.returncode == 1
    assert "arg-type" in result.stdout
    assert "import-untyped" not in result.stdout


def test_workspace_overlays_with_installed_sdk(installed: Path, tmp_path: Path) -> None:
    """Independent checker processes share one installed SDK, never one mutable overlay."""
    from concurrent.futures import ThreadPoolExecutor
    from uuid import UUID

    from catalog_overlay import write_overlay
    from transflow._catalog_prototype import PrototypeSnapshot

    installed_files = {
        str(path): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in installed.parent.parent.rglob("*")
        if path.is_file() and not path.is_symlink()
    }

    def exercise(index: int, name: str, absent: str) -> None:
        workspace = tmp_path / name
        workspace.mkdir()
        snapshot = PrototypeSnapshot(
            str(UUID(int=index)),
            (
                (f"raw/{name}", str(UUID(int=100))),
                (f"raw/{name}/daily", str(UUID(int=101))),
            ),
        )
        overlay = write_overlay(workspace, snapshot)
        config = workspace / "mypy.ini"
        config.write_text(
            "[mypy]\nstrict = True\n"
            f"mypy_path = {overlay}\npython_executable = {installed}\n"
            "cache_dir = .mypy_cache\n"
        )
        consumer = workspace / "consumer.py"
        consumer.write_text(
            "from transflow import (PROTOCOL_VERSION, ProtocolCompatibilityError, "
            "ProtocolVersion, __version__, require_protocol)\n"
            "from transflow.catalog import C\n"
            "from transflow._catalog_prototype import resolve_reference\n"
            "from transflow.testing import catalog_context\n"
            "from transflow_worker import __version__ as worker_version\n"
            "require_protocol(ProtocolVersion(1, 0))\n"
            "from transflow import Input, Output, transform\n"
            "@transform(value=Input('raw/value'), output=Output('out'))\n"
            "def ordinary(value: int) -> int: return value + 1\n"
            "answer: int = ordinary(1)\n"
            "assert isinstance(__version__, str)\n"
            "assert isinstance(PROTOCOL_VERSION, ProtocolVersion)\n"
            "assert issubclass(ProtocolCompatibilityError, RuntimeError)\n"
            "assert worker_version == __version__\n"
            f"resolve_reference(C.raw.{name}, expected_fingerprint={snapshot.fingerprint!r})\n"
            f"reveal_type(C.raw.{name}.daily)\n"
        )
        command = [sys.executable, "-m", "mypy", "--config-file", str(config), str(consumer)]
        result = run(command, workspace)
        assert 'Revealed type is "transflow.catalog._Node' in result.stdout
        consumer.write_text(consumer.read_text() + f"C.raw.{absent}\nProtocolVersion('wrong', 0)\n")
        result = run(command, workspace, ok=False)
        assert result.returncode == 1
        assert "attr-defined" in result.stdout and "arg-type" in result.stdout
        assert "import-untyped" not in result.stdout and "import-not-found" not in result.stdout
        # Read actual mypy member tables, the typed candidates behind C.raw completion.
        probe = """
import sys
from mypy.build import build
from mypy.main import process_options
sources, options = process_options(["--config-file", sys.argv[1], sys.argv[2]])
options.preserve_asts = True
options.incremental = False
result = build(sources, options)
module = result.graph["transflow.catalog"].tree
root = module.names["C"].node.type.type
raw = root.names["raw"].node.func.type.ret_type.type
print(",".join(sorted(name for name in raw.names if not name.startswith("_"))))
"""
        candidates = run([sys.executable, "-c", probe, str(config), str(consumer)], workspace)
        assert candidates.stdout.strip() == name
        # Even a deliberately misplaced typing-only path cannot shadow the installed package.
        runtime = f"""
import sys
sys.path.insert(0, {str(overlay)!r})
import transflow
import transflow.catalog
from pathlib import Path
from transflow import ProtocolVersion, require_protocol
from transflow._catalog_prototype import PrototypeSnapshot, resolve_reference
from transflow.testing import catalog_context
from transflow.catalog import C
assert not Path(transflow.__file__).is_relative_to({str(workspace)!r})
assert not Path(transflow.catalog.__file__).is_relative_to({str(workspace)!r})
require_protocol(ProtocolVersion(1, 0))
snapshot = PrototypeSnapshot({snapshot.workspace_id!r}, {snapshot.entries!r})
with catalog_context(snapshot, expected_fingerprint={snapshot.fingerprint!r}):
    assert dir(C.raw) == [{name!r}]
    ref = resolve_reference(C.raw.{name}, expected_fingerprint=snapshot.fingerprint)
    assert ref.path == "raw/{name}"
"""
        run([str(installed), "-I", "-B", "-c", runtime], workspace)
        assert not list(overlay.rglob("*.py"))

    with ThreadPoolExecutor(max_workers=2) as executor:
        futures = [
            executor.submit(exercise, 1, "orders", "invoices"),
            executor.submit(exercise, 2, "invoices", "orders"),
        ]
        for future in futures:
            future.result(timeout=90)
    assert installed_files == {
        str(path): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in installed.parent.parent.rglob("*")
        if path.is_file() and not path.is_symlink()
    }


def test_installed_wire_validators_need_no_repository(installed: Path, tmp_path: Path) -> None:
    script = """
from transflow_worker.canonical import canonical_json, file_digest
from io import BytesIO
assert canonical_json({"z": -0.0, "a": "18446744073709551615"}) == (
    b'{"a":"18446744073709551615","z":0}'
)
assert file_digest(BytesIO(b"abc")).hex == (
    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
)
from transflow_worker._wire_validators import validate_WireValue
from transflow_worker.wire import ProtocolError, MAX_FRAME_BYTES
validate_WireValue({"type": "u64", "value": "18446744073709551615"})
try:
    validate_WireValue({"type": "u64", "value": "18446744073709551616"})
except ProtocolError:
    pass
else:
    raise AssertionError("overflow accepted")
assert MAX_FRAME_BYTES == 1048576
print("wire-ok")
"""
    result = run([str(installed), "-I", "-c", script], tmp_path)
    assert result.stdout.strip() == "wire-ok"
