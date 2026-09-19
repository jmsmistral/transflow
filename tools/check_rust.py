"""Check Cargo crate boundaries against architecture 2.2 and qualified pins."""

from __future__ import annotations

import argparse
from collections import Counter
import json
from pathlib import Path
import subprocess
import sys
import tomllib


INTERNAL = {
    "tf-domain": set(),
    "tf-protocol": {"tf-domain"},
    "tf-catalog": {"tf-domain", "tf-protocol"},
    "tf-plan": {"tf-domain", "tf-catalog"},
    "tf-store": {"tf-domain", "tf-protocol"},
    "tf-exec": {"tf-domain", "tf-protocol", "tf-plan", "tf-store"},
    "tf-schedule": {"tf-domain", "tf-plan", "tf-store"},
    "tf-api": {"tf-domain", "tf-protocol", "tf-catalog", "tf-plan", "tf-exec", "tf-schedule"},
    "tf-lsp": {"tf-domain", "tf-catalog", "tf-api"},
    "transflow": {"tf-domain", "tf-protocol", "tf-catalog", "tf-plan", "tf-store", "tf-exec", "tf-schedule", "tf-api", "tf-lsp"},
}

# New external dependencies require an explicit boundary and qualification review.
# The domain crate intentionally starts with no third-party dependencies.
EXTERNAL = {
    "tf-domain": set(),
    "tf-protocol": {"serde", "serde_json", "schemars", "thiserror"},
    "tf-catalog": {"serde", "serde_json", "thiserror"},
    "tf-plan": {"thiserror"},
    "tf-store": {"sqlx", "libsqlite3-sys", "arrow-array", "arrow-schema", "parquet", "fs4", "serde", "serde_json", "thiserror"},
    "tf-exec": {"tokio", "fs4", "tracing", "thiserror"},
    "tf-schedule": {"chrono", "chrono-tz", "serde", "thiserror"},
    "tf-api": {"axum", "tower", "serde", "serde_json", "tokio", "tracing", "thiserror"},
    "tf-lsp": {"serde", "serde_json", "tokio", "tracing", "thiserror"},
    "transflow": {"clap", "thiserror", "tokio", "tracing"},
}

# T007's real SQLite integration fixture needs an executor, only in tests.
DEV_EXTERNAL = {"tf-store": {"tokio"}}


def validate_graph(metadata: dict, qualified: dict[str, str]) -> list[str]:
    """Check every declared dependency, including optional/build/dev/target ones."""
    errors = []
    members = set(metadata["workspace_members"])
    packages = [package for package in metadata["packages"] if package["id"] in members]
    names = Counter(package["name"] for package in packages)
    if set(names) != set(INTERNAL) or any(count != 1 for count in names.values()):
        errors.append("Workspace must contain exactly the ten specified crates")
    root = Path(metadata["workspace_root"]).resolve()
    edges = {name: [] for name in names}
    for package in packages:
        name = package["name"]
        expected_manifest = root / "crates" / name / "Cargo.toml"
        if Path(package["manifest_path"]).resolve() != expected_manifest:
            errors.append(f"{name}: manifest must live in crates/{name}")
        if package["edition"] != "2024" or package["rust_version"] != "1.98.1":
            errors.append(f"{name}: edition/MSRV drift from the qualified baseline")
        if package["publish"] != []:
            errors.append(f"{name}: bootstrap packages must have publish=false")
        for dependency in package["dependencies"]:
            target = dependency["name"]  # Actual package name, not its import alias.
            if target in INTERNAL:
                edges[name].append(target)
                if target not in INTERNAL.get(name, set()):
                    errors.append(f"Forbidden crate dependency: {name} -> {target}")
                expected_path = root / "crates" / target
                if dependency.get("source") is not None or Path(dependency.get("path", "")).resolve() != expected_path:
                    errors.append(f"{name} -> {target}: must use the local workspace crate")
            else:
                allowed = EXTERNAL.get(name, set())
                if dependency.get("kind") == "dev":
                    allowed = allowed | DEV_EXTERNAL.get(name, set())
                if target not in allowed:
                    errors.append(f"Unapproved external dependency: {name} -> {target}")
                if dependency.get("source") != "registry+https://github.com/rust-lang/crates.io-index":
                    errors.append(f"{name} -> {target}: unqualified dependency source")
                if dependency["req"] != qualified.get(target):
                    errors.append(f"{name} -> {target}: version differs from the exact qualification pin")

    visited = set()
    active = []

    def visit(name: str) -> None:
        if name in active:
            errors.append("Crate dependency cycle: " + " -> ".join([*active, name]))
            return
        if name in visited:
            return
        active.append(name)
        for target in edges.get(name, []):
            visit(target)
        active.pop()
        visited.add(name)

    for name in sorted(edges):
        visit(name)
    return errors


def validate_checkout(root: Path) -> list[str]:
    metadata = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--offline", "--no-deps"],
        cwd=root, capture_output=True, text=True, check=False,
    )
    if metadata.returncode:
        return ["Cargo metadata failed: " + metadata.stderr.strip()]
    probe = tomllib.loads((root / "tools/qualification/rust/Cargo.toml").read_text())
    pins = {name: entry if isinstance(entry, str) else entry["version"] for name, entry in probe["dependencies"].items()}
    errors = validate_graph(json.loads(metadata.stdout), pins)
    for name in INTERNAL:
        manifest = root / "crates" / name / "Cargo.toml"
        if not manifest.is_file():
            errors.append(f"Missing manifest: {name}")
            continue
        data = tomllib.loads(manifest.read_text())
        if data.get("lints") != {"workspace": True}:
            errors.append(f"{name}: must inherit workspace lints without overrides")
    baseline = tomllib.loads((root / "tools/qualification/rust/Cargo.lock").read_text())
    qualified_versions = {(p["name"], p["version"], p.get("source"), p.get("checksum")) for p in baseline["package"]}
    application = tomllib.loads((root / "Cargo.lock").read_text())
    for package in application["package"]:
        if package.get("source") is not None:
            identity = (package["name"], package["version"], package["source"], package.get("checksum"))
            if identity not in qualified_versions:
                errors.append(f"Unqualified locked package: {package['name']} {package['version']}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    try:
        errors = validate_checkout(args.root.resolve())
    except (OSError, ValueError, KeyError) as exc:
        errors = [f"Cannot check Rust workspace: {exc}"]
    if errors:
        print("Rust boundary checks failed:\n" + "\n".join(f"  - {error}" for error in errors), file=sys.stderr)
        return 1
    print("Rust boundaries passed: ten crates, acyclic dependencies, inherited lints and qualified lock entries.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
