"""The local editor package has a finite, reproducible, non-runtime payload."""

import json
import shutil
from pathlib import Path
from zipfile import ZipFile

import pytest
from package_vscode import PAYLOAD, package


@pytest.fixture
def extension(tmp_path: Path) -> Path:
    source = Path(__file__).resolve().parents[2] / "editors/vscode"
    destination = tmp_path / "extension"
    shutil.copytree(source, destination)
    return destination


def test_vsix_excludes_unlisted_files_and_is_reproducible(extension: Path, tmp_path: Path) -> None:
    (extension / "private-notes.txt").write_text("Do not package workspace data")
    first = package(extension, tmp_path / "one.vsix")
    second = package(extension, tmp_path / "two.vsix")
    assert first.read_bytes() == second.read_bytes()
    with ZipFile(first) as archive:
        assert set(archive.namelist()) == {
            "extension.vsixmanifest",
            "[Content_Types].xml",
            *(f"extension/{name}" for name in PAYLOAD),
        }
        assert json.loads(archive.read("extension/package.json"))["private"] is True


def test_vsix_refuses_symlinked_payload_and_keeps_existing_output(
    extension: Path, tmp_path: Path
) -> None:
    target = tmp_path / "outside.cjs"
    target.write_text("outside")
    (extension / "extension.cjs").unlink()
    (extension / "extension.cjs").symlink_to(target)
    output = tmp_path / "existing.vsix"
    output.write_bytes(b"existing artifact")
    with pytest.raises(ValueError, match="regular file"):
        package(extension, output)
    assert output.read_bytes() == b"existing artifact"


def test_vsix_refuses_unsupported_runtime_dependencies(extension: Path, tmp_path: Path) -> None:
    path = extension / "package.json"
    manifest = json.loads(path.read_text())
    manifest["dependencies"] = {"unexpected-runtime": "1.0.0"}
    path.write_text(json.dumps(manifest))
    with pytest.raises(ValueError, match="dependency-free"):
        package(extension, tmp_path / "rejected.vsix")
