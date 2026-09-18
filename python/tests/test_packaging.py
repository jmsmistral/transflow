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
        "protocol_major": 0,
        "protocol_minor": 0,
        "wire_protocol_implemented": False,
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
        ["compatibility", "--protocol-major", "1"],
        ["compatibility", "--protocol-minor", "1"],
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
        "require_protocol(ProtocolVersion(0, 0))\n"
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
    valid.write_text('from transflow import ProtocolVersion\nProtocolVersion("wrong", 0)\n')
    result = run(command, tmp_path, ok=False)
    assert result.returncode == 1
    assert "arg-type" in result.stdout
    assert "import-untyped" not in result.stdout
