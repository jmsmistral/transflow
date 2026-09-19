"""Resolve/install synthetic wheels entirely offline using the pinned pip tools."""

import base64
import csv
import hashlib
import io
import json
import os
import subprocess
import sys
import venv
from pathlib import Path
from zipfile import ZipFile, ZipInfo


def run(command, root, *, success=True):
    environment = {
        "PATH": os.defpath,
        "PIP_CONFIG_FILE": os.devnull,
        "PIP_NO_INDEX": "1",
        "PIP_DISABLE_PIP_VERSION_CHECK": "1",
        "PYTHONDONTWRITEBYTECODE": "1",
        "XDG_CACHE_HOME": str(root / "cache"),
    }
    result = subprocess.run(
        command, cwd=root, env=environment, capture_output=True, text=True, timeout=30, check=False
    )
    if success and result.returncode:
        raise RuntimeError(result.stdout + result.stderr)
    return result


def wheel(root: Path, name: str, version: str, requires=None):
    metadata = f"{name}-{version}.dist-info"
    content = {
        f"{name}/__init__.py": f'__version__ = "{version}"\n'.encode(),
        f"{metadata}/METADATA": (
            f"Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n"
            + (f"Requires-Dist: {requires}\n" if requires else "")
        ).encode(),
        f"{metadata}/WHEEL": (
            b"Wheel-Version: 1.0\nGenerator: transflow-fixture\n"
            b"Root-Is-Purelib: true\nTag: py3-none-any\n"
        ),
    }
    record = io.StringIO()
    writer = csv.writer(record, lineterminator="\n")
    for name_in_wheel, data in content.items():
        digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
        writer.writerow([name_in_wheel, f"sha256={digest}", len(data)])
    writer.writerow([f"{metadata}/RECORD", "", ""])
    content[f"{metadata}/RECORD"] = record.getvalue().encode()
    path = root / f"{name}-{version}-py3-none-any.whl"
    with ZipFile(path, "w") as archive:
        for name_in_wheel, data in content.items():
            archive.writestr(ZipInfo(name_in_wheel, date_time=(2020, 1, 1, 0, 0, 0)), data)
    return path


def qualify(root: Path):
    wheelhouse = root / "wheelhouse"
    wheelhouse.mkdir()
    leaf = wheel(wheelhouse, "transflow_fixture_leaf", "1.0.0")
    wheel(wheelhouse, "transflow_fixture_leaf", "2.0.0")
    parent = wheel(wheelhouse, "transflow_fixture_root", "1.0.0", "transflow-fixture-leaf>=1,<2")
    source = root / "requirements.in"
    source.write_text("transflow-fixture-root==1.0.0\n")
    lock = root / "requirements.lock"
    command = [
        sys.executable,
        "-m",
        "piptools",
        "compile",
        "--no-index",
        "--find-links",
        str(wheelhouse),
        "--cache-dir",
        str(root / "cache"),
        "--generate-hashes",
        "--strip-extras",
        "--no-header",
        "--no-emit-index-url",
        "--no-emit-find-links",
        "--output-file",
        str(lock),
        str(source),
    ]
    run(command, root)
    locked = lock.read_text()
    run(command, root)
    assert locked == lock.read_text()
    assert "transflow-fixture-leaf==1.0.0" in locked
    assert "==2.0.0" not in locked
    hashes = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in (leaf, parent)}
    assert all(digest in locked for digest in hashes.values())
    environment = root / "installed"
    venv.EnvBuilder(with_pip=False).create(environment)
    python = environment / "bin/python"
    install = [
        sys.executable,
        "-m",
        "pip",
        "--python",
        str(python),
        "install",
        "--no-index",
        "--find-links",
        str(wheelhouse),
        "--require-hashes",
        "--only-binary=:all:",
        "-r",
        str(lock),
    ]
    run(install, root)
    run([sys.executable, "-m", "pip", "--python", str(python), "check"], root)
    installed = run(
        [
            str(python),
            "-I",
            "-B",
            "-c",
            "import json,transflow_fixture_leaf,transflow_fixture_root; "
            "print(json.dumps([transflow_fixture_leaf.__version__, "
            "transflow_fixture_root.__version__]))",
        ],
        root,
    )
    assert json.loads(installed.stdout) == ["1.0.0", "1.0.0"]
    before = {
        str(path.relative_to(environment)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in environment.rglob("*")
        if path.is_file() and not path.is_symlink()
    }
    # Leave the zip readable but change bytes: pip must reject its previously locked hash.
    with ZipFile(parent, "a") as archive:
        archive.writestr("tampered.txt", "untrusted fixture change")
    rejected = run([*install, "--force-reinstall", "--no-cache-dir"], root, success=False)
    assert rejected.returncode != 0 and "DO NOT MATCH THE HASHES" in rejected.stderr
    after = {
        str(path.relative_to(environment)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in environment.rglob("*")
        if path.is_file() and not path.is_symlink()
    }
    assert before == after
    return {
        "failed_install_preserves_environment": True,
        "offline_resolution": True,
        "transitive_constraint_respected": True,
        "repeat_lock_identical": True,
        "clean_install": True,
        "tampering_rejected": True,
        "fixture_wheel_hashes": hashes,
    }
