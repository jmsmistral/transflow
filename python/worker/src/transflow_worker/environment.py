"""Explicit pip environment preparation; also embedded in the Rust CLI before SDK installation."""

import argparse
import hashlib
import importlib.metadata as metadata
import json
import os
import re
import shutil
import subprocess
import sys
import sysconfig
import tempfile
import uuid
import venv
from pathlib import Path
from typing import cast
from zipfile import ZipFile

PIP_VERSION = "26.2.1"
RESOLVER_VERSION = "7.6.1"
LOCK_PREFIX = "# transflow-lock-v1 "
MAX_TEXT = 16 * 1024 * 1024
# Canonical validation is a runtime service, even when user code imports only Polars.
# Keep this standard-library module self-contained: the CLI embeds it before install.
MANAGED_PACKAGES = {"duckdb": "1.5.5"}


class EnvironmentError(ValueError):
    """Safe fixed diagnostic; never echo package manager output or credential-bearing URLs."""


def _canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def _digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def _object(value: object) -> dict[str, object]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise EnvironmentError("Environment metadata is malformed; synchronize explicitly")
    return cast(dict[str, object], value)


def _text(path: Path) -> str:
    if path.is_symlink() or not path.is_file() or path.stat().st_size > MAX_TEXT:
        raise EnvironmentError("A required environment file is missing, unsafe or oversized")
    with path.open("rb") as stream:
        value = stream.read(MAX_TEXT + 1)
    if len(value) > MAX_TEXT:
        raise EnvironmentError("Environment metadata exceeds its size limit")
    return value.decode("utf-8")


def _relative(root: Path, name: str) -> Path:
    relative = Path(name)
    if not name or relative.is_absolute() or any(p in {"", ".", ".."} for p in name.split("/")):
        raise EnvironmentError("Environment inputs must be ordinary workspace-relative files")
    path = root / relative
    for item in [path, *path.parents]:
        if item == root:
            break
        if item.is_symlink():
            raise EnvironmentError("Environment paths cannot traverse symlinks")
    if not path.resolve().is_relative_to(root):
        raise EnvironmentError("Environment path escaped the workspace")
    return path


def _atomic(path: Path, value: bytes) -> None:
    descriptor, name = tempfile.mkstemp(prefix=".transflow-env-", dir=path.parent)
    temporary = Path(name)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(value)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        descriptor = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
    finally:
        temporary.unlink(missing_ok=True)


def target_identity() -> dict[str, object]:
    """Interpreter target used for resolution; native binary bytes are recorded separately."""
    return {
        "implementation": sys.implementation.name,
        "version": list(sys.version_info[:3]),
        "cache_tag": sys.implementation.cache_tag,
        "soabi": sysconfig.get_config_var("SOABI"),
        "platform": sysconfig.get_platform(),
    }


def _tooling(minor: str) -> None:
    if minor != f"{sys.version_info.major}.{sys.version_info.minor}" or minor != "3.14":
        raise EnvironmentError("Select a Python interpreter matching configured 3.14")
    for name, version in [("pip", PIP_VERSION), ("pip-tools", RESOLVER_VERSION)]:
        try:
            actual = metadata.version(name)
        except metadata.PackageNotFoundError as exc:
            raise EnvironmentError(
                "Prepare the qualified pip and pip-tools tooling explicitly"
            ) from exc
        if actual != version:
            raise EnvironmentError(
                "Python tooling differs from the qualified pip/pip-tools versions"
            )


def _run(arguments: list[str], root: Path, *, offline: bool) -> None:
    environment = {
        "PATH": os.defpath,
        "PIP_CONFIG_FILE": os.devnull,
        "PIP_DISABLE_PIP_VERSION_CHECK": "1",
        "PYTHONDONTWRITEBYTECODE": "1",
        "PYTHONNOUSERSITE": "1",
        "PIP_CACHE_DIR": str(root / "pip-cache"),
    }
    if offline:
        environment["PIP_NO_INDEX"] = "1"
    try:
        result = subprocess.run(
            arguments,
            cwd=root,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=600,
            check=False,
        )
    except subprocess.TimeoutExpired as exc:
        raise EnvironmentError(
            "Explicit package preparation timed out; inspect availability and retry"
        ) from exc
    if result.returncode:
        raise EnvironmentError(
            "Package preparation failed; check declared packages, wheel availability and hashes"
        )


def _name(value: str) -> str:
    return re.sub(r"[-_.]+", "-", value).lower()


def _requirements(text: str) -> None:
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("-") or any(marker in line for marker in ("@", "://", "file:")):
            raise EnvironmentError(
                "Use index requirements; editable, direct URL and nested inputs are unsupported"
            )
        name = re.match(r"[A-Za-z0-9_.-]+", line)
        if name is None or _name(name.group()) == "transflow":
            raise EnvironmentError(
                "The matched Transflow wheel is supplied separately; never resolve it from an index"
            )


def _lock_pins(text: str) -> dict[str, str]:
    pins: dict[str, str] = {}
    for entry in text.replace("\\\n", " ").splitlines():
        entry = entry.strip()
        if not entry or entry.startswith("#"):
            continue
        match = re.fullmatch(
            r"([A-Za-z0-9_.-]+)==([^\s;]+)(?:\s*;[^#]+?)?((?:\s+--hash=sha256:[0-9a-f]{64})+)\s*",
            entry,
        )
        if match is None:
            raise EnvironmentError(
                "Dependency lock must contain exact versions and SHA-256 hashes only; run env lock"
            )
        name = _name(match.group(1))
        if name == "transflow" or name in pins:
            raise EnvironmentError(
                "Lock repeats a package or attempts to replace the matched Transflow wheel"
            )
        pins[name] = match.group(2)
    return pins


def lock_environment(
    root: Path,
    requirements: str,
    lock: str,
    minor: str,
    *,
    wheelhouse: Path | None = None,
    offline: bool = False,
) -> dict[str, object]:
    """Resolve using pinned tooling and replace the lock only after guards and validation pass."""
    root = root.resolve()
    _tooling(minor)
    source = _relative(root, requirements)
    destination = _relative(root, lock)
    source_bytes = _text(source)
    _requirements(source_bytes)
    old = _text(destination) if destination.exists() else None
    with tempfile.TemporaryDirectory(
        prefix="env-lock-", dir=_relative(root, ".transflow/runtime")
    ) as temporary:
        stage = Path(temporary)
        captured = stage / "requirements.in"
        captured.write_text(
            source_bytes + "\n" + "\n".join(f"{n}=={v}" for n, v in MANAGED_PACKAGES.items()) + "\n"
        )
        resolved = stage / "requirements.lock"
        arguments = [
            sys.executable,
            "-I",
            "-m",
            "piptools",
            "compile",
            "--cache-dir",
            str(stage / "resolver-cache"),
            "--generate-hashes",
            "--strip-extras",
            "--allow-unsafe",
            "--no-header",
            "--no-annotate",
            "--no-emit-options",
            "--no-emit-index-url",
            "--no-emit-find-links",
            "--no-emit-trusted-host",
            "--pip-args=--only-binary=:all:",
            "--output-file",
            str(resolved),
        ]
        if offline:
            arguments.append("--no-index")
        if wheelhouse is not None:
            arguments.extend(["--find-links", str(wheelhouse.resolve())])
        arguments.append(str(captured))
        _run(arguments, stage, offline=offline)
        text = _text(resolved)
        pins = _lock_pins(text)
        if (
            _text(source) != source_bytes
            or (_text(destination) if destination.exists() else None) != old
        ):
            raise EnvironmentError(
                "Dependency declarations or lock changed during resolution; retry"
            )
        header = {
            "target": target_identity(),
            "pip": PIP_VERSION,
            "resolver": RESOLVER_VERSION,
            "requirements_sha256": hashlib.sha256(source_bytes.encode()).hexdigest(),
        }
        _atomic(destination, LOCK_PREFIX.encode() + _canonical(header) + b"\n" + text.encode())
    return {"action": "lock", "packages": len(pins), "lock_sha256": _digest(destination)}


def _wheel(path: Path, expected: str) -> str:
    from email.parser import Parser

    if path.is_symlink() or not path.is_file() or path.suffix != ".whl":
        raise EnvironmentError("Supply the explicit application-matched Transflow wheel")
    with ZipFile(path) as archive:
        names = archive.namelist()
        members = [name for name in names if name.endswith(".dist-info/METADATA")]
        if len(members) != 1 or archive.getinfo(members[0]).file_size > MAX_TEXT:
            raise EnvironmentError("Matched wheel metadata is invalid")
        info = Parser().parsestr(archive.read(members[0]).decode())
        if (
            info.get("Name") != "transflow"
            or info.get("Version") != expected
            or "transflow/__init__.py" not in names
            or "transflow_worker/__init__.py" not in names
        ):
            raise EnvironmentError(
                "Wheel must contain the application-matched SDK and worker distribution"
            )
    return _digest(path)


def inspect_environment(environment: Path) -> dict[str, object]:
    """Hash actual installed state without importing packages or executing target .pth files."""
    environment = environment.resolve()
    site = (
        environment
        / "lib"
        / f"python{sys.version_info.major}.{sys.version_info.minor}"
        / "site-packages"
    )
    if not site.is_dir() or site.is_symlink():
        raise EnvironmentError("Managed environment is missing; run env sync")
    packages: dict[str, str] = {}
    for distribution in metadata.distributions(path=[str(site)]):
        raw_name = distribution.metadata.get("Name")
        if not raw_name:
            raise EnvironmentError("Installed package metadata is incomplete; run env sync")
        name = _name(raw_name)
        if name in packages or distribution.files is None:
            raise EnvironmentError("Installed packages have duplicate or missing file inventories")
        direct = distribution.read_text("direct_url.json")
        if direct is not None and _object(json.loads(direct)).get("dir_info"):
            raise EnvironmentError(
                "Mutable local/editable dependencies are not captured; use a locked wheel"
            )
        packages[name] = distribution.version
    files: list[dict[str, object]] = []
    for base, directories, names in os.walk(site, followlinks=False):
        directories[:] = sorted(d for d in directories if d != "__pycache__")
        for directory in directories:
            if (Path(base) / directory).is_symlink():
                raise EnvironmentError("Managed package directories cannot be symlinks")
        for name in sorted(names):
            path = Path(base) / name
            if path.suffix == ".pyc":
                continue
            if path.is_symlink() or not path.is_file():
                raise EnvironmentError("Managed package files cannot be links or special files")
            files.append(
                {
                    "path": str(path.relative_to(environment)),
                    "sha256": _digest(path),
                    "bytes": path.stat().st_size,
                }
            )
            if len(files) > 100_000:
                raise EnvironmentError("Installed environment exceeds the file inventory limit")
    binary = environment / "bin/python"
    config_hash = _digest(environment / "pyvenv.cfg")
    entrypoints = {
        p.name: _digest(p) for p in sorted((environment / "bin").iterdir()) if p.is_file()
    }

    semantic = {
        "target": target_identity(),
        "interpreter_sha256": _digest(binary),
        "base_interpreter_sha256": _digest(Path(sys.executable).resolve()),
        "venv_configuration_sha256": config_hash,
        "entrypoints": entrypoints,
        "packages": dict(sorted(packages.items())),
        "files": files,
    }
    return {
        "format_version": 1,
        "fingerprint": hashlib.sha256(
            b"transflow.environment.v1\x00" + _canonical(semantic)
        ).hexdigest(),
        **semantic,
    }


def _lock_state(root: Path, requirements: str, lock: str) -> tuple[str, dict[str, str]]:
    text = _text(_relative(root, lock))
    header, separator, body = text.partition("\n")
    if not separator or not header.startswith(LOCK_PREFIX):
        raise EnvironmentError("Dependency lock has no qualified target metadata; run env lock")
    info = _object(json.loads(header[len(LOCK_PREFIX) :]))
    if (
        info.get("target") != target_identity()
        or info.get("pip") != PIP_VERSION
        or info.get("resolver") != RESOLVER_VERSION
    ):
        raise EnvironmentError(
            "Dependency lock targets a different interpreter/platform or resolver; run env lock"
        )
    if info.get("requirements_sha256") != _digest(_relative(root, requirements)):
        raise EnvironmentError("Dependency input changed after locking; run env lock then env sync")
    pins = _lock_pins(body)
    if any(pins.get(name) != version for name, version in MANAGED_PACKAGES.items()):
        raise EnvironmentError(
            "Dependency lock lacks the qualified managed check engine; run env lock then env sync"
        )
    return text, pins


def sync_environment(
    root: Path,
    requirements: str,
    lock: str,
    minor: str,
    runtime_wheel: Path,
    expected_version: str,
    *,
    wheelhouse: Path | None = None,
    offline: bool = False,
) -> dict[str, object]:
    """Build a fresh managed environment and switch readiness only after complete verification."""
    root = root.resolve()
    _tooling(minor)
    text, pins = _lock_state(root, requirements, lock)
    runtime_wheel = runtime_wheel.absolute()
    wheel_hash = _wheel(runtime_wheel, expected_version)
    runtime = _relative(root, ".transflow/runtime")
    parent = runtime / "environments"
    if parent.is_symlink():
        raise EnvironmentError("Managed environment directory cannot be a symlink")
    parent.mkdir(mode=0o700, exist_ok=True)
    specification = hashlib.sha256(
        _canonical(
            {
                "target": target_identity(),
                "lock_sha256": hashlib.sha256(text.encode()).hexdigest(),
                "wheel_sha256": wheel_hash,
                "interpreter_sha256": _digest(Path(sys.executable).resolve()),
            }
        )
    ).hexdigest()
    specification_root = parent / specification
    if specification_root.is_symlink():
        raise EnvironmentError("Environment specification directory cannot be a symlink")
    specification_root.mkdir(mode=0o700, exist_ok=True)
    destination = specification_root / uuid.uuid4().hex
    destination.mkdir(mode=0o700)
    ready = False
    try:
        venv.EnvBuilder(with_pip=False).create(destination)
        python = destination / "bin/python"
        with tempfile.TemporaryDirectory(prefix="env-sync-", dir=runtime) as temporary:
            stage = Path(temporary)
            captured_lock = stage / "requirements.lock"
            captured_lock.write_text(text)
            captured_wheel = stage / runtime_wheel.name
            shutil.copyfile(runtime_wheel, captured_wheel)
            if _digest(captured_wheel) != wheel_hash:
                raise EnvironmentError("Matched wheel changed while copying; retry")
            arguments = [
                sys.executable,
                "-I",
                "-m",
                "pip",
                "--python",
                str(python),
                "install",
                "--no-compile",
                "--only-binary=:all:",
                "--require-hashes",
                "-r",
                str(captured_lock),
            ]
            if offline:
                arguments.append("--no-index")
            if wheelhouse is not None:
                arguments.extend(["--find-links", str(wheelhouse.resolve())])
            _run(arguments, stage, offline=offline)
            _run(
                [
                    sys.executable,
                    "-I",
                    "-m",
                    "pip",
                    "--python",
                    str(python),
                    "install",
                    "--no-index",
                    "--no-deps",
                    "--no-compile",
                    str(captured_wheel),
                ],
                stage,
                offline=True,
            )
            _run(
                [sys.executable, "-I", "-m", "pip", "--python", str(python), "check"],
                stage,
                offline=True,
            )
        _run(
            [str(python), "-I", "-B", "-m", "transflow_worker", "compatibility"],
            runtime,
            offline=True,
        )
        record = inspect_environment(destination)
        if record["packages"] != {**pins, "transflow": expected_version}:
            raise EnvironmentError(
                "Installed package set differs from the locked dependencies and matched runtime"
            )
        if _text(_relative(root, lock)) != text or _digest(runtime_wheel) != wheel_hash:
            raise EnvironmentError("Lock or matched wheel changed during synchronization; retry")
        _lock_state(root, requirements, lock)
        # Imported module files are included in the package byte inventory, not only METADATA.
        record.update(
            {
                "environment": str(destination.relative_to(parent)),
                "lock_sha256": hashlib.sha256(text.encode()).hexdigest(),
                "runtime_wheel_sha256": wheel_hash,
                "runtime_version": expected_version,
                "interpreter_locator": str(Path(sys.executable).absolute()),
            }
        )
        _atomic(destination / "environment.json", _canonical(record))
        _atomic(runtime / "environment.json", _canonical(record))
        ready = True
        return {"action": "sync", "fingerprint": record["fingerprint"], "packages": len(pins) + 1}
    finally:
        if not ready:
            shutil.rmtree(destination)


def check_environment(root: Path, requirements: str, lock: str, minor: str) -> dict[str, object]:
    """Fail on lock, interpreter or installed-byte drift; never install or update anything."""
    if minor != f"{sys.version_info.major}.{sys.version_info.minor}":
        raise EnvironmentError("Use the configured Python interpreter")
    root = root.resolve()
    text, _ = _lock_state(root, requirements, lock)
    runtime = _relative(root, ".transflow/runtime")
    record = _object(json.loads(_text(runtime / "environment.json")))
    name = record.get("environment")
    if not isinstance(name, str) or re.fullmatch(r"[0-9a-f]{64}/[0-9a-f]{32}", name) is None:
        raise EnvironmentError("Managed environment locator is invalid; run env sync")
    environment = _relative(root, f".transflow/runtime/environments/{name}")
    if environment.is_symlink():
        raise EnvironmentError("Managed environment locator cannot be a symlink")
    actual = inspect_environment(environment)
    if (
        record.get("fingerprint") != actual["fingerprint"]
        or record.get("lock_sha256") != hashlib.sha256(text.encode()).hexdigest()
        or record.get("interpreter_locator") != str(Path(sys.executable).absolute())
    ):
        raise EnvironmentError(
            "Installed environment or interpreter drifted; run env sync before reuse"
        )
    return {
        "action": "check",
        "fingerprint": actual["fingerprint"],
        "packages": len(cast(dict[str, str], actual["packages"])),
        "interpreter": str(environment / "bin/python"),
        "runtime_version": cast(dict[str, str], actual["packages"])["transflow"],
    }


def main() -> int:
    """Private helper entry point; no user imports or automatic tooling installation."""
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("request")
    args = parser.parse_args()
    try:
        request = _object(json.loads(args.request))
        root = Path(str(request["root"]))
        common = (root, str(request["requirements"]), str(request["lock"]), str(request["minor"]))
        wheelhouse = Path(str(request["wheelhouse"])) if request.get("wheelhouse") else None
        offline = request.get("offline") is True
        if request["action"] == "lock":
            result = lock_environment(*common, wheelhouse=wheelhouse, offline=offline)
        elif request["action"] == "sync":
            if not request.get("runtime_wheel"):
                raise EnvironmentError(
                    "Supply --runtime-wheel with the matched local wheel; no index fallback"
                )
            result = sync_environment(
                *common,
                Path(str(request["runtime_wheel"])),
                str(request["runtime_version"]),
                wheelhouse=wheelhouse,
                offline=offline,
            )
        elif request["action"] == "check":
            result = check_environment(*common)
        else:
            raise EnvironmentError("Unknown environment operation")
        print(json.dumps({"ok": True, "result": result}, sort_keys=True))
        return 0
    except (EnvironmentError, OSError, ValueError, KeyError) as error:
        message = (
            str(error)
            if isinstance(error, EnvironmentError)
            else "Cannot read valid environment state; correct the files and retry"
        )
        print(json.dumps({"ok": False, "error": message}))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
