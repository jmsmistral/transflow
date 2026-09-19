"""Explicit license inventory refresh: offline Cargo metadata and public PyPI metadata."""

import io
import json
import subprocess
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path

from baseline import canonical, digest, inventory, require

ROOT = Path(__file__).resolve().parents[2]


def fetch(url):
    with urllib.request.urlopen(url, timeout=45) as response:
        data = response.read(32 * 1024 * 1024 + 1)
    require(len(data) <= 32 * 1024 * 1024, "License metadata response exceeds size limit")
    return data


def python_licenses(item):
    url = f"https://pypi.org/pypi/{item['name']}/{item['version']}/json"
    data = json.loads(fetch(url))
    info = data["info"]
    if info.get("license_expression"):
        return [info["license_expression"]]
    if info.get("license") and info["license"].strip() not in (
        "UNKNOWN",
        "NOASSERTION",
    ):
        return [info["license"].strip()]
    classifiers = [
        c for c in info["classifiers"] if c.startswith("License ::") and c.count(" :: ") >= 2
    ]
    if classifiers:
        return classifiers
    # Some older packages declare their license only in wheel license files.
    wheels = sorted(
        (p for p in data["urls"] if p["filename"].endswith(".whl")),
        key=lambda p: p["filename"],
    )
    require(wheels, f"No license declaration or wheel: {item['id']}")
    wheel = wheels[0]
    parsed = urllib.parse.urlparse(wheel["url"])
    require(
        parsed.scheme == "https" and parsed.hostname == "files.pythonhosted.org",
        "Unexpected wheel host",
    )
    checksum = wheel["digests"]["sha256"]
    require(
        any(f"--hash=sha256:{checksum}" in (ROOT / lock).read_text() for lock in item["locks"]),
        "License wheel hash is absent from the lockfiles",
    )
    raw = fetch(wheel["url"])
    require(digest(raw) == checksum, "License wheel checksum mismatch")
    with zipfile.ZipFile(io.BytesIO(raw)) as archive:
        files = [
            p
            for p in archive.infolist()
            if ".dist-info/" in p.filename
            and Path(p.filename).name.upper().startswith(("LICENSE", "COPYING"))
        ]
        require(
            files and all(p.file_size <= 1024 * 1024 for p in files),
            "Missing/oversized wheel license",
        )
        return sorted(archive.read(p).decode().strip() for p in files)


def main():
    data = inventory(ROOT)
    rust = {}
    for manifest in ("Cargo.toml", "tools/qualification/rust/Cargo.toml"):
        result = subprocess.check_output(
            [
                "cargo",
                "metadata",
                "--locked",
                "--offline",
                "--format-version",
                "1",
                "--manifest-path",
                str(ROOT / manifest),
            ],
            cwd=ROOT,
        )
        for item in json.loads(result)["packages"]:
            if item.get("source"):
                rust[f"crates.io:{item['name']}@{item['version']}"] = item.get("license")
    for item in data["packages"]:
        if item["ecosystem"] == "PyPI":
            item["licenses"] = python_licenses(item)
        elif item["ecosystem"] == "crates.io":
            item["licenses"] = [rust.get(item["id"])]
        require(
            item["licenses"] and all(isinstance(v, str) and v.strip() for v in item["licenses"]),
            f"Missing license: {item['id']}",
        )
    (ROOT / "security/dependencies.json").write_bytes(canonical(data))
    print(
        f"Wrote declared licenses for {len(data['packages'])} locked package versions; "
        "advisory refresh is now required."
    )


if __name__ == "__main__":
    main()
