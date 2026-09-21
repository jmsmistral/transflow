"""Real offline resolution/installation, actual byte drift and failure preservation."""

import csv
import hashlib
import io
import json
import sys
from pathlib import Path
from typing import cast
from zipfile import ZipFile

import pytest
from test_packaging import wheel as wheel
from transflow import __version__
from transflow_worker.environment import (
    EnvironmentError,
    check_environment,
    inspect_environment,
    lock_environment,
    sync_environment,
)

MINOR = f"{sys.version_info.major}.{sys.version_info.minor}"


def fixture_wheel(root: Path, name: str, version: str, requires: str | None = None) -> Path:
    import base64

    dist = f"{name}-{version}.dist-info"
    files = {
        f"{name}/__init__.py": f"VALUE = {version!r}\n".encode(),
        f"{dist}/METADATA": (
            f"Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n"
            + (f"Requires-Dist: {requires}\n" if requires else "")
        ).encode(),
        f"{dist}/WHEEL": (
            b"Wheel-Version: 1.0\nGenerator: transflow-test\n"
            b"Root-Is-Purelib: true\nTag: py3-none-any\n"
        ),
    }
    record = io.StringIO()
    writer = csv.writer(record, lineterminator="\n")
    for member, data in files.items():
        digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
        writer.writerow([member, f"sha256={digest}", len(data)])
    writer.writerow([f"{dist}/RECORD", "", ""])
    files[f"{dist}/RECORD"] = record.getvalue().encode()
    path = root / f"{name}-{version}-py3-none-any.whl"
    with ZipFile(path, "w") as archive:
        for name_in_wheel, data in files.items():
            archive.writestr(name_in_wheel, data)
    return path


@pytest.fixture
def workspace(tmp_path: Path) -> Path:
    (tmp_path / ".transflow/runtime").mkdir(parents=True)
    (tmp_path / "wheels").mkdir()
    fixture_wheel(tmp_path / "wheels", "transflow_fixture_leaf", "1.0.0")
    fixture_wheel(tmp_path / "wheels", "transflow_fixture_leaf", "2.0.0")
    fixture_wheel(
        tmp_path / "wheels", "transflow_fixture_root", "1.0.0", "transflow-fixture-leaf>=1,<2"
    )
    (tmp_path / "requirements.in").write_text("transflow-fixture-root==1.0.0\n")
    return tmp_path


def lock(root: Path) -> dict[str, object]:
    return lock_environment(
        root,
        "requirements.in",
        "requirements.lock",
        MINOR,
        wheelhouse=root / "wheels",
        offline=True,
    )


def sync(root: Path, wheel: Path) -> dict[str, object]:
    return sync_environment(
        root,
        "requirements.in",
        "requirements.lock",
        MINOR,
        wheel,
        __version__,
        wheelhouse=root / "wheels",
        offline=True,
    )


def check(root: Path) -> dict[str, object]:
    return check_environment(root, "requirements.in", "requirements.lock", MINOR)


def active(root: Path) -> Path:
    record = cast(
        dict[str, str], json.loads((root / ".transflow/runtime/environment.json").read_text())
    )
    return root / ".transflow/runtime/environments" / record["environment"]


def test_offline_cold_setup_locks_transitive_hashes_and_verifies_actual_state(
    workspace: Path, wheel: Path
) -> None:
    assert lock(workspace)["packages"] == 2
    original = (workspace / "requirements.lock").read_bytes()
    assert "transflow-fixture-leaf==1.0.0" in original.decode()
    assert lock(workspace)["packages"] == 2
    assert (workspace / "requirements.lock").read_bytes() == original
    report = sync(workspace, wheel)
    assert report["packages"] == 3
    assert check(workspace)["fingerprint"] == report["fingerprint"]
    installed = inspect_environment(active(workspace))
    assert installed["packages"] == {
        "transflow": __version__,
        "transflow-fixture-leaf": "1.0.0",
        "transflow-fixture-root": "1.0.0",
    }


def test_installed_byte_and_interpreter_drift_fail_without_installation(
    workspace: Path, wheel: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    lock(workspace)
    sync(workspace, wheel)
    package = (
        active(workspace)
        / "lib"
        / f"python{MINOR}"
        / "site-packages/transflow_fixture_leaf/__init__.py"
    )
    package.write_text("VALUE='tampered'\n")

    def forbidden(*args: object, **kwargs: object) -> None:
        raise AssertionError("drift check must never invoke pip")

    monkeypatch.setattr("transflow_worker.environment._run", forbidden)
    with pytest.raises(EnvironmentError, match="drifted"):
        check(workspace)


def test_failed_sync_preserves_last_ready_environment(workspace: Path, wheel: Path) -> None:
    lock(workspace)
    sync(workspace, wheel)
    before = (workspace / ".transflow/runtime/environment.json").read_bytes()
    before_directories = set((workspace / ".transflow/runtime/environments").iterdir())
    with (workspace / "wheels/transflow_fixture_leaf-1.0.0-py3-none-any.whl").open("ab") as stream:
        stream.write(b"tampered")
    with pytest.raises(EnvironmentError, match="Package preparation failed"):
        sync(workspace, wheel)
    assert (workspace / ".transflow/runtime/environment.json").read_bytes() == before
    assert set((workspace / ".transflow/runtime/environments").iterdir()) == before_directories
    assert check(workspace)["action"] == "check"


@pytest.mark.parametrize(
    "requirements",
    ["transflow==1.0\n", "-e ../mutable\n", "package @ file:///tmp/wheel.whl\n", "-r other.in\n"],
)
def test_no_index_substitution_or_mutable_inputs(workspace: Path, requirements: str) -> None:
    (workspace / "requirements.in").write_text(requirements)
    with pytest.raises(EnvironmentError):
        lock(workspace)
    assert not (workspace / "requirements.lock").exists()


def test_missing_runtime_and_changed_input_do_not_create_ready_state(workspace: Path) -> None:
    lock(workspace)
    with pytest.raises(EnvironmentError, match="matched"):
        sync(workspace, workspace / "missing.whl")
    assert not (workspace / ".transflow/runtime/environment.json").exists()
    (workspace / "requirements.in").write_text("transflow-fixture-leaf==2.0.0\n")
    with pytest.raises(EnvironmentError, match="changed"):
        check(workspace)


def test_editable_metadata_and_untracked_installed_files_fail_reuse(
    workspace: Path, wheel: Path
) -> None:
    lock(workspace)
    sync(workspace, wheel)
    site = active(workspace) / "lib" / f"python{MINOR}" / "site-packages"
    (site / "untracked.py").write_text("pass\n")
    with pytest.raises(EnvironmentError, match="drifted"):
        check(workspace)
    (site / "untracked.py").unlink()
    (site / "transflow_fixture_leaf-1.0.0.dist-info/direct_url.json").write_text(
        '{"url":"file:///synthetic/mutable","dir_info":{"editable":true}}'
    )
    with pytest.raises(EnvironmentError, match="Mutable"):
        check(workspace)


def test_venv_configuration_drift_and_pth_code_are_not_executed(
    workspace: Path, wheel: Path
) -> None:
    lock(workspace)
    sync(workspace, wheel)
    environment = active(workspace)
    config = environment / "pyvenv.cfg"
    old = config.read_text()
    config.write_text(old + "# changed\n")
    with pytest.raises(EnvironmentError, match="drifted"):
        check(workspace)
    config.write_text(old)
    marker = workspace / "unexpected-execution"
    site = environment / "lib" / f"python{MINOR}" / "site-packages"
    (site / "unexpected.pth").write_text(f"import pathlib; pathlib.Path({str(marker)!r}).touch()\n")
    with pytest.raises(EnvironmentError, match="drifted"):
        check(workspace)
    assert not marker.exists()


def test_missing_sdk_metadata_is_drift_and_never_triggers_index_install(
    workspace: Path, wheel: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    lock(workspace)
    sync(workspace, wheel)
    import shutil

    site = active(workspace) / "lib" / f"python{MINOR}" / "site-packages"
    shutil.rmtree(site / f"transflow-{__version__}.dist-info")

    def forbidden(*args: object, **kwargs: object) -> None:
        raise AssertionError("missing metadata must not trigger an installation")

    monkeypatch.setattr("transflow_worker.environment._run", forbidden)
    with pytest.raises(EnvironmentError, match="drifted"):
        check(workspace)


@pytest.mark.parametrize("minor", ["3.12", "3.15"])
def test_unsupported_interpreter_is_rejected_before_resolution(tmp_path: Path, minor: str) -> None:
    with pytest.raises(EnvironmentError, match="configured 3.14"):
        lock_environment(tmp_path, "requirements.in", "requirements.lock", minor, offline=True)
    assert not (tmp_path / "requirements.lock").exists()
