"""Offline, standard-library checks for repository privacy and locked dependencies."""

from __future__ import annotations

import datetime as dt
import hashlib
import json
import re
import subprocess
import tomllib
from pathlib import Path

MAX_FILE_BYTES = 16 * 1024 * 1024
LOCKS = (
    "Cargo.lock",
    "tools/qualification/rust/Cargo.lock",
    "web/package-lock.json",
    "tools/qualification/web/package-lock.json",
    "python/dev-py314.lock",
    "tools/qualification/python/resolver.lock",
    "tools/qualification/python/py314.lock",
)
RULES = {
    "github-token": rb"\b(?:gh[pousr]_[A-Za-z0-9]{36,255}|github_pat_[A-Za-z0-9_]{22,255})\b",
    "aws-access-key": rb"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b",
    "private-key": rb"-----BEGIN (?:RSA |EC |OPENSSH |DSA |ENCRYPTED )?PRIVATE KEY-----",
    "assigned-secret": (
        rb"(?i)\b(?:password|api[_-]?key|access[_-]?token|client[_-]?secret)"
        rb"""\s*[:=]\s*["']([A-Za-z0-9+/=_-]{20,})["']"""
    ),
}


class Failure(ValueError):
    """A failed or incomplete check; never a successful empty result."""


def require(condition: object, message: str) -> None:
    if not condition:
        raise Failure(message)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def canonical(data: object) -> bytes:
    return (json.dumps(data, sort_keys=True, indent=2, ensure_ascii=True) + "\n").encode()


def read_json(path: Path):
    return json.loads(path.read_text())


def git_files(root: Path) -> list[str]:
    actual = subprocess.check_output(["git", "-C", str(root), "rev-parse", "--show-toplevel"])
    require(
        Path(actual.decode().strip()).resolve() == root.resolve(),
        "Expected an explicit Git repository root",
    )
    raw = subprocess.check_output(
        [
            "git",
            "-C",
            str(root),
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ]
    )
    return sorted({name.decode("utf-8") for name in raw.split(b"\0") if name})


def safe_path(root: Path, name: str) -> Path:
    path = Path(name)
    require(
        not path.is_absolute() and ".." not in path.parts and name != ".",
        f"Unsafe relative path: {name!r}",
    )
    cursor = root
    for part in path.parts:
        cursor /= part
        require(not cursor.is_symlink(), f"Symlink cannot be audited: {name!r}")
    return cursor


def scan_bytes(data: bytes, name: str) -> list[dict]:
    findings = []
    for rule, pattern in RULES.items():
        for match in re.finditer(pattern, data):
            findings.append(
                {
                    "path": name,
                    "line": data[: match.start()].count(b"\n") + 1,
                    "rule": rule,
                    "fingerprint": digest(match.group())[:16],
                }
            )
    return findings


def scan_repository(root: Path, private_names: set[str]) -> tuple[int, list[dict]]:
    count, findings = 0, []
    for name in git_files(root):
        parts = Path(name).parts
        require(
            Path(name).name.casefold() not in private_names
            and ".DS_Store" not in parts
            and not name.casefold().startswith("references/screenshots/"),
            f"Private path in public inventory: {name!r}",
        )
        path = safe_path(root, name)
        if not path.exists():  # Tracked deletions are fine after the path/privacy checks.
            continue
        require(path.is_file(), f"Non-file cannot be audited: {name!r}")
        with path.open("rb") as stream:
            data = stream.read(MAX_FILE_BYTES + 1)
        require(len(data) <= MAX_FILE_BYTES, f"File exceeds privacy scan limit: {name!r}")
        findings.extend(scan_bytes(data, name))
        count += 1
    return count, findings


def private_names(spec: Path) -> set[str]:
    mapping = safe_path(spec, "references/SCREENSHOTS.md").read_text()
    names = re.findall(r"^\|\s*`U\d{2}`\s*\|\s*`([^`]+)`\s*\|", mapping, re.MULTILINE)
    require(
        len(names) == 20 and len(set(names)) == 20,
        "Expected the canonical U01-U20 screenshot mapping",
    )
    return {name.casefold() for name in names}


def package_id(ecosystem: str, name: str, version: str) -> str:
    if ecosystem == "PyPI":
        name = re.sub(r"[-_.]+", "-", name).lower()
    require(bool(re.fullmatch(r"[A-Za-z0-9@_./+-]+", name)), "Invalid package name")
    require(bool(re.fullmatch(r"[A-Za-z0-9_.+-]+", version)), "Invalid package version")
    return f"{ecosystem}:{name}@{version}"


def inventory(root: Path) -> dict:
    candidates = git_files(root)
    discovered = {
        p
        for p in candidates
        if p.endswith(".lock")
        or Path(p).name in ("package-lock.json", "npm-shrinkwrap.json", "yarn.lock")
    }
    require(
        discovered == set(LOCKS),
        f"Lock coverage changed; review the inventory parser: {sorted(discovered ^ set(LOCKS))}",
    )
    packages: dict[str, dict] = {}
    locks = {}

    def add(eco, name, version, origin, license_text=None, deprecated=None):
        key = package_id(eco, name, version)
        item = packages.setdefault(
            key,
            {
                "id": key,
                "ecosystem": eco,
                "name": key[len(eco) + 1 :].rsplit("@", 1)[0],
                "version": version,
                "locks": [],
                "licenses": [],
                "deprecations": [],
            },
        )
        if origin not in item["locks"]:
            item["locks"].append(origin)
        if license_text:
            require(isinstance(license_text, str), f"Invalid license declaration: {key}")
            if license_text not in item["licenses"]:
                item["licenses"].append(license_text)
        if deprecated and deprecated not in item["deprecations"]:
            item["deprecations"].append(deprecated)

    for name in LOCKS:
        raw = safe_path(root, name).read_bytes()
        locks[name] = digest(raw)
        if name.endswith("Cargo.lock"):
            for item in tomllib.loads(raw.decode())["package"]:
                if "source" not in item:
                    # The Rust boundary gate validates workspace/path crates.
                    continue
                require(
                    item["source"] == "registry+https://github.com/rust-lang/crates.io-index"
                    and re.fullmatch("[a-f0-9]{64}", item.get("checksum", "")),
                    f"Unqualified Cargo source/checksum: {name}",
                )
                add("crates.io", item["name"], item["version"], name)
        elif name.endswith("package-lock.json"):
            lock = json.loads(raw)
            require(lock["lockfileVersion"] == 3, f"Unsupported npm lock version: {name}")
            for location, item in lock["packages"].items():
                if not location:
                    continue
                require(
                    not item.get("link")
                    and item.get("resolved", "").startswith("https://registry.npmjs.org/")
                    and re.fullmatch(r"sha512-[A-Za-z0-9+/]+={0,2}", item.get("integrity", "")),
                    f"Unqualified npm source/integrity: {name}:{location}",
                )
                add(
                    "npm",
                    item.get("name") or location.rsplit("node_modules/", 1)[-1],
                    item["version"],
                    name,
                    item.get("license"),
                    item.get("deprecated"),
                )
        else:
            logical = raw.decode().replace("\\\n", " ")
            for line in logical.splitlines():
                line = line.split("#", 1)[0].strip()
                if not line:
                    continue
                match = re.fullmatch(
                    r"([A-Za-z0-9_.-]+)==([A-Za-z0-9_.+!-]+)\s+((?:--hash=sha256:[a-f0-9]{64}\s*)+)",
                    line,
                )
                require(match, f"Expected exact hash-locked Python requirement: {name}")
                add("PyPI", match[1], match[2], name)
    for item in packages.values():
        for field in ("locks", "licenses", "deprecations"):
            item[field].sort()
    return {
        "schema": 1,
        "locks": locks,
        "packages": sorted(packages.values(), key=lambda p: p["id"]),
    }


def check_inventory(root: Path) -> dict:
    current = inventory(root)
    saved = read_json(root / "security/dependencies.json")
    require(
        saved.get("schema") == 1 and saved.get("locks") == current["locks"],
        "Dependency inventory is stale; run refresh-inventory.py",
    )
    require(
        len(saved["packages"]) == len(current["packages"]),
        "Dependency inventory coverage differs",
    )
    for actual, recorded in zip(current["packages"], saved["packages"], strict=True):
        for key in ("id", "ecosystem", "name", "version", "locks", "deprecations"):
            require(
                actual[key] == recorded[key],
                f"Dependency inventory drift: {actual['id']} ({key})",
            )
        licenses = recorded.get("licenses")
        require(
            isinstance(licenses, list)
            and licenses
            and all(
                isinstance(s, str) and s.strip() and s not in ("UNKNOWN", "NOASSERTION")
                for s in licenses
            ),
            f"Missing declared license: {actual['id']}",
        )
        if actual["ecosystem"] == "npm":
            require(actual["licenses"] == licenses, f"npm license drift: {actual['id']}")
    return saved


def apply_exceptions(findings: set[str], policy: dict, today: dt.date) -> list[str]:
    require(
        set(policy) == {"schema", "exceptions"} and policy["schema"] == 1,
        "Unsupported exception policy",
    )
    accepted = set()
    for item in policy["exceptions"]:
        require(
            set(item) == {"finding", "owner", "reason", "expires"},
            "Invalid exception fields",
        )
        key = item["finding"]
        require(
            isinstance(key, str)
            and key.startswith(("deprecated:npm:", "advisory:"))
            and "*" not in key,
            "Exceptions must name an exact advisory/package/version or deprecation",
        )
        require(
            key not in accepted and key in findings,
            f"Duplicate or unused exception: {key}",
        )
        require(
            isinstance(item["owner"], str)
            and item["owner"].strip()
            and isinstance(item["reason"], str)
            and len(item["reason"].strip()) >= 20,
            f"Exception needs owner and reason: {key}",
        )
        require(dt.date.fromisoformat(item["expires"]) > today, f"Expired exception: {key}")
        accepted.add(key)
    return sorted(findings - accepted)


def check_advisories(data: dict, dependencies: dict, today: dt.date) -> set[str]:
    require(
        data.get("schema") == 1 and data.get("provider") == "https://api.osv.dev/v1/querybatch",
        "Unsupported advisory report",
    )
    require(
        data.get("inventory_sha256") == digest(canonical(dependencies)),
        "Advisory report does not match inventory",
    )
    age = (today - dt.date.fromisoformat(data["checked_at"][:10])).days
    require(0 <= age <= 7, "Advisory report is stale or future-dated; explicitly refresh it")
    ids = [p["id"] for p in dependencies["packages"]]
    require(data.get("queried") == ids, "Advisory query coverage is incomplete")
    findings = set()
    for item in data["findings"]:
        require(
            set(item) == {"package", "advisory"}
            and item["package"] in ids
            and re.fullmatch(r"[A-Za-z0-9_.-]+", item["advisory"]),
            "Malformed advisory finding",
        )
        findings.add(f"advisory:{item['advisory']}:{item['package']}")
    return findings
