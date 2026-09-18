"""Resolve hash-locked binary wheels for each target. Does not execute foreign code."""

from __future__ import annotations

import argparse
import importlib.metadata
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from urllib.parse import unquote, urlsplit


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if importlib.metadata.version("pip") != "26.2.1":
        parser.error("Use the qualified pip 26.2.1 resolver environment")
    locks = Path(__file__).resolve().parent
    platforms = {"macos-arm64": ["macosx_15_0_arm64"]}
    for arch in ("x86_64", "aarch64"):
        platforms[f"linux-{arch}"] = [f"manylinux_2_{minor}_{arch}" for minor in range(28, 16, -1)] + [f"manylinux2014_{arch}"]
    results = []
    with tempfile.TemporaryDirectory(prefix="transflow-wheel-matrix-") as directory:
        for version in ("3.13", "3.14"):
            for target, tags in platforms.items():
                report_path = Path(directory) / f"{version}-{target}.json"
                command = [
                    sys.executable, "-m", "pip", "--isolated", "install", "--dry-run", "--ignore-installed",
                    "--disable-pip-version-check", "--only-binary=:all:", "--require-hashes",
                    "--index-url", "https://pypi.org/simple", "--python-version", version,
                    "--implementation", "cp", "--abi", "cp" + version.replace(".", ""),
                    "--abi", "abi3", "--abi", "none", "--report", str(report_path),
                    "-r", str(locks / f"py{version.replace('.', '')}.lock"),
                ]
                for tag in tags:
                    command.extend(["--platform", tag])
                try:
                    completed = subprocess.run(command, capture_output=True, text=True, timeout=300, check=False)
                    if completed.returncode:
                        raise RuntimeError(completed.stderr[-4000:])
                    report = json.loads(report_path.read_text())
                    wheels = [{
                        "name": item["metadata"]["name"], "version": item["metadata"]["version"],
                        "filename": unquote(Path(urlsplit(item["download_info"]["url"]).path).name),
                        "sha256": item["download_info"]["archive_info"]["hashes"]["sha256"],
                    } for item in report["install"]]
                    if not wheels or any(not item["filename"].endswith(".whl") for item in wheels):
                        raise RuntimeError("Resolution did not return only binary wheels")
                    results.append({"python": version, "target": target, "status": "wheels_resolved", "wheels": wheels})
                    print(f"{version} {target}: {len(wheels)} hash-verified wheel candidates", flush=True)
                except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as exc:
                    results.append({"python": version, "target": target, "status": "failed", "error": str(exc)})
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps({"scope": "Wheel resolution only; not native execution or full marker qualification", "pip": "26.2.1", "results": results}, indent=2) + "\n")
    return int(any(result["status"] == "failed" for result in results))


if __name__ == "__main__":
    raise SystemExit(main())
