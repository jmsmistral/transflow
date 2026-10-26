"""Build the dependency-free local Transflow VSIX from an explicit payload list."""

from __future__ import annotations

import argparse
import json
import os
import tempfile
from pathlib import Path
from typing import Any
from xml.etree import ElementTree as ET
from zipfile import ZIP_STORED, ZipFile, ZipInfo

PAYLOAD = ("README.md", "extension.cjs", "package.json", "transport.cjs")


def package(source: Path, output: Path) -> Path:
    """Package only the local client; refuse unsupported runtime dependencies."""
    contents: dict[str, bytes] = {}
    for name in PAYLOAD:
        path = source / name
        if path.is_symlink() or not path.is_file():
            raise ValueError(f"Extension payload must be a regular file: {name}")
        contents[f"extension/{name}"] = path.read_bytes()
    manifest: dict[str, Any] = json.loads(contents["extension/package.json"])
    if (
        manifest.get("name") != "transflow"
        or manifest.get("publisher") != "transflow-local"
        or manifest.get("private") is not True
        or manifest.get("main") != "./extension.cjs"
        or manifest.get("dependencies")
        or manifest.get("optionalDependencies")
    ):
        raise ValueError("This packager supports only the private dependency-free Transflow client")

    root = ET.Element(
        "PackageManifest",
        {"Version": "2.0.0", "xmlns": "http://schemas.microsoft.com/developer/vsx-schema/2011"},
    )
    metadata = ET.SubElement(root, "Metadata")
    ET.SubElement(
        metadata,
        "Identity",
        {
            "Language": "en-US",
            "Id": manifest["name"],
            "Version": manifest["version"],
            "Publisher": manifest["publisher"],
        },
    )
    ET.SubElement(metadata, "DisplayName").text = manifest["displayName"]
    ET.SubElement(metadata, "Description").text = manifest["description"]
    ET.SubElement(metadata, "Categories").text = ",".join(manifest["categories"])
    properties = ET.SubElement(metadata, "Properties")
    for name, value in (
        ("Microsoft.VisualStudio.Code.Engine", manifest["engines"]["vscode"]),
        ("Microsoft.VisualStudio.Code.ExtensionKind", "workspace"),
        ("Microsoft.VisualStudio.Services.GitHubFlavoredMarkdown", "true"),
    ):
        ET.SubElement(properties, "Property", {"Id": name, "Value": value})
    installation = ET.SubElement(root, "Installation")
    ET.SubElement(installation, "InstallationTarget", {"Id": "Microsoft.VisualStudio.Code"})
    ET.SubElement(root, "Dependencies")
    assets = ET.SubElement(root, "Assets")
    for kind, asset_path in (
        ("Microsoft.VisualStudio.Code.Manifest", "extension/package.json"),
        ("Microsoft.VisualStudio.Services.Content.Details", "extension/README.md"),
    ):
        ET.SubElement(assets, "Asset", {"Type": kind, "Path": asset_path, "Addressable": "true"})
    contents["extension.vsixmanifest"] = ET.tostring(root, encoding="utf-8", xml_declaration=True)

    types = ET.Element(
        "Types", {"xmlns": "http://schemas.openxmlformats.org/package/2006/content-types"}
    )
    for extension, mime in (
        ("cjs", "application/javascript"),
        ("json", "application/json"),
        ("md", "text/markdown"),
        ("vsixmanifest", "text/xml"),
    ):
        ET.SubElement(types, "Default", {"Extension": extension, "ContentType": mime})
    contents["[Content_Types].xml"] = ET.tostring(types, encoding="utf-8", xml_declaration=True)

    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=output.parent, suffix=".vsix", delete=False) as temporary:
        temporary_path = Path(temporary.name)
    try:
        with ZipFile(temporary_path, "w", compression=ZIP_STORED) as archive:
            for name, value in sorted(contents.items()):
                info = ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                info.create_system = 3
                info.external_attr = 0o100644 << 16
                archive.writestr(info, value)
        os.replace(temporary_path, output)
    finally:
        temporary_path.unlink(missing_ok=True)
    return output


def main() -> None:
    """Build a local installable artifact without network access or publishing."""
    repository = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output", type=Path, default=repository / "target/editors/transflow-0.0.0.vsix"
    )
    args = parser.parse_args()
    print(package(repository / "editors/vscode", args.output))


if __name__ == "__main__":
    main()
