"""Render T010 mypy overlays; all generated files are typing-only and workspace-local."""

from __future__ import annotations

import hashlib
import json
import tempfile
from pathlib import Path

from transflow._catalog_prototype import CatalogContextError, PrototypeSnapshot

# Explicit forwarding preserves the SDK root exports without creating runtime Python files.
ROOT_STUB = """from transflow._version import __version__ as __version__
from transflow.compatibility import (
    PROTOCOL_VERSION as PROTOCOL_VERSION,
    ProtocolCompatibilityError as ProtocolCompatibilityError,
    ProtocolVersion as ProtocolVersion,
    require_protocol as require_protocol,
)
from transflow._declaration_values import (
    DeclarationError as DeclarationError, Parameter as Parameter,
)
from transflow.declarations import (
    Branch as Branch, Check as Check, Input as Input, Output as Output,
    TransformContext as TransformContext, transform as transform,
    source_transform as source_transform,
)
"""


def render(snapshot: PrototypeSnapshot) -> dict[str, str]:
    paths = {""}
    for path, _ in snapshot.entries:
        parts = path.split("/")
        paths.update("/".join(parts[:index]) for index in range(1, len(parts) + 1))
    classes = {path: f"_Node{index}" for index, path in enumerate(sorted(paths))}
    lines = [
        f"# Catalogue fingerprint: {snapshot.fingerprint}",
        "from transflow._catalog_prototype import CatalogNode",
        "",
    ]
    for path in sorted(paths):
        lines.append(f"class {classes[path]}(CatalogNode):")
        children = snapshot.children(path)
        for child in children:
            target = f"{path}/{child}" if path else child
            lines.extend(["    @property", f"    def {child}(self) -> {classes[target]}: ..."])
        if not children:
            lines.append("    pass")
        lines.append("")
    lines.append(f"C: {classes['']}")
    return {"transflow/__init__.pyi": ROOT_STUB, "transflow/catalog.pyi": "\n".join(lines) + "\n"}


def _safe(root: Path, path: Path) -> None:
    if not path.is_relative_to(root):
        raise ValueError("Overlay path escaped its explicit workspace")
    if root.is_symlink() or any(
        part.is_symlink() for part in (path, *path.parents) if part.is_relative_to(root)
    ):
        raise ValueError("Overlay paths may not traverse symlinks")


def verify(overlay: Path, snapshot: PrototypeSnapshot) -> None:
    expected = render(snapshot)
    metadata_path = overlay / "manifest.json"
    if overlay.is_symlink() or metadata_path.is_symlink():
        raise ValueError("Overlay may not be a symlink")
    paths = list(overlay.rglob("*"))
    if any(path.is_symlink() for path in paths):
        raise ValueError("Overlay contents may not contain symlinks")
    metadata = json.loads(metadata_path.read_text())
    if metadata != {
        "fingerprint": snapshot.fingerprint,
        "files": {
            name: hashlib.sha256(text.encode()).hexdigest() for name, text in expected.items()
        },
    }:
        raise CatalogContextError("Editor overlay fingerprint or manifest is stale")
    actual = {str(path.relative_to(overlay)) for path in paths if path.is_file()}
    if actual != {*expected, "manifest.json"}:
        raise CatalogContextError("Editor overlay contains unexpected files")
    for name, text in expected.items():
        path = overlay / name
        if path.read_text() != text:
            raise CatalogContextError("Editor overlay content differs from its captured catalogue")


def write_overlay(workspace: Path, snapshot: PrototypeSnapshot) -> Path:
    """Create a new immutable generation atomically; refuse stale/tampered existing output."""
    if not workspace.is_absolute() or not workspace.is_dir():
        raise ValueError("An explicit existing absolute workspace is required")
    parent = workspace / ".transflow/runtime/generated" / snapshot.fingerprint
    destination = parent / "type-stubs"
    _safe(workspace, destination)
    parent.mkdir(parents=True, exist_ok=True)
    if destination.exists():
        verify(destination, snapshot)
        return destination
    files = render(snapshot)
    with tempfile.TemporaryDirectory(prefix=".overlay-", dir=parent) as staging:
        staged = Path(staging)
        for name, text in files.items():
            path = staged / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        (staged / "manifest.json").write_text(
            json.dumps(
                {
                    "fingerprint": snapshot.fingerprint,
                    "files": {
                        name: hashlib.sha256(text.encode()).hexdigest()
                        for name, text in files.items()
                    },
                },
                sort_keys=True,
                indent=2,
            )
            + "\n"
        )
        try:
            staged.rename(destination)
        except OSError:
            if not destination.exists():
                raise
            verify(destination, snapshot)
    verify(destination, snapshot)
    return destination
