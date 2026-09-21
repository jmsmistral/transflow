"""Captured-source discovery in a fresh installed worker; trusted imports execute code."""

from __future__ import annotations

import hashlib
import importlib
import importlib.abc
import importlib.machinery
import inspect
import json
import os
import socket
import stat
import sys
import threading
import traceback
from dataclasses import asdict
from pathlib import Path
from types import CodeType, ModuleType
from typing import Any, BinaryIO, cast

from transflow._catalog import CatalogSnapshot, DatasetRef, _bind_worker
from transflow.declarations import Branch, Check, Declaration, get_declaration

from .canonical import DigestKind, content_digest
from .compatibility import check_compatibility
from .source_index import ModuleEntry, ModuleIndex, ModuleIndexError
from .wire import ControlFrame, ProtocolError, _decode, validate_document, write_frame

MAX_METADATA = 16 * 1024 * 1024
MAX_SOURCE_TOTAL = 256 * 1024 * 1024


class DiscoveryError(ValueError):
    """Safe explanation and source location; arbitrary exception text is not exported."""

    def __init__(
        self,
        code: str,
        message: str,
        path: str | None = None,
        line: int | None = None,
        exception_type: str | None = None,
    ) -> None:
        super().__init__(message)
        self.diagnostic = {
            "format_version": 1,
            "code": code,
            "message": message,
            "path": path,
            "line": line,
            "exception_type": exception_type,
        }


def _read(path: Path, limit: int, *, private: bool = False) -> bytes:
    for part in (path, *path.parents):
        if part.is_symlink():
            raise DiscoveryError("unsafe_path", "Discovery files cannot traverse symlinks")
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or (private and info.st_mode & 0o077):
            raise DiscoveryError("unsafe_path", "Discovery request must be a private regular file")
        if info.st_size > limit:
            raise DiscoveryError("limit", "Discovery input exceeds its size limit")
        data = stream.read(limit + 1)
        if len(data) > limit:
            raise DiscoveryError("limit", "Discovery input exceeds its size limit")
        return data


def _directory(value: str, *, private: bool = False) -> Path:
    path = Path(value)
    if not path.is_absolute() or path.resolve() != path or not path.is_dir():
        raise DiscoveryError("unsafe_path", "Discovery requires canonical absolute directories")
    if private and path.stat().st_mode & 0o077:
        raise DiscoveryError("unsafe_path", "Discovery control/results directory must be private")
    return path


class CapturedLoader(importlib.abc.Loader):
    def __init__(self, code: CodeType) -> None:
        self.code = code

    def create_module(self, spec: importlib.machinery.ModuleSpec) -> None:
        return None

    def exec_module(self, module: ModuleType) -> None:
        # This is deliberate execution of trusted captured module code, never data eval.
        exec(self.code, module.__dict__)


class CapturedFinder(importlib.abc.MetaPathFinder):
    def __init__(self, root: Path, index: ModuleIndex, codes: dict[str, CodeType]) -> None:
        self.root, self.entries, self.codes = root, {e.name: e for e in index.modules}, codes
        self.top = {e.name.split(".")[0] for e in index.modules}

    def find_spec(
        self, fullname: str, path: object = None, target: object = None
    ) -> importlib.machinery.ModuleSpec | None:
        entry = self.entries.get(fullname)
        if entry is None:
            if fullname.split(".")[0] in self.top:
                raise ModuleNotFoundError(
                    "Module is absent from the captured allowlist", name=fullname
                )
            return None
        location = str(self.root / entry.path)
        if entry.kind == "namespace":
            spec = importlib.machinery.ModuleSpec(fullname, None, is_package=True)
            spec.submodule_search_locations = [location]
        else:
            spec = importlib.machinery.ModuleSpec(
                fullname,
                CapturedLoader(self.codes[fullname]),
                origin=location,
                is_package=entry.kind == "package",
            )
            spec.has_location = True
            if entry.kind == "package":
                spec.submodule_search_locations = [str(Path(location).parent)]
        return spec


def _ref(value: str | DatasetRef) -> dict[str, object]:
    if isinstance(value, str):
        return {"form": "string", "value": value}
    return {
        "form": "bound",
        "workspace_id": value.workspace_id,
        "dataset_id": value.dataset_id,
        "path": value.path,
        "catalog_fingerprint": value.fingerprint,
    }


def _check(check: Check) -> dict[str, object]:
    value = asdict(check)
    if check.sample_rows is not None:
        value["sample_rows"] = str(check.sample_rows)
    return value


def _declaration(value: Declaration, entry: ModuleEntry) -> dict[str, object]:
    return {
        "module": value.module,
        "function": value.qualname,
        "path": entry.path,
        "line": value.line,
        "source": value.source,
        "engine": value.engine,
        "inputs": [
            {
                "alias": alias,
                "ref": _ref(binding.ref),
                "branch": {
                    "kind": "omitted"
                    if binding.branch is None
                    else "current"
                    if binding.branch is Branch.CURRENT
                    else "named",
                    "name": binding.branch if isinstance(binding.branch, str) else None,
                },
                "stop_branch_fallback": binding.stop_branch_fallback,
                "role": binding.role,
                "checks": [_check(check) for check in binding.checks],
            }
            for alias, binding in value.inputs
        ],
        "output": {
            "ref": _ref(value.output.ref),
            "schema": value.output.schema,
            "checks": [_check(check) for check in value.output.checks],
        },
        "parameters": [
            {"name": name, "logical_type": p.logical_type, "default": p.default}
            for name, p in value.params
        ],
        "wall_timeout_seconds": (
            str(dict(value.resources)["wall_timeout_seconds"]) if value.resources else None
        ),
        "cache": value.cache,
        "refresh": value.refresh,
        "secret_refs": list(value.secret_refs),
        "lineage_json": value.lineage_json.decode() if value.lineage_json is not None else None,
    }


def collect(request: dict[str, Any]) -> dict[str, object]:
    """One fresh interpreter only. The coordinator verifies installed environment drift first."""
    validate_document("DiscoveryRequestV1", request)
    if request["protocol"]["major"] != 1 or request["protocol"]["minor"] != 0:
        raise DiscoveryError("version", "Discovery protocol version is unsupported")
    catalog = request["catalog"]
    projection = {
        k: v for k, v in catalog.items() if k not in {"catalog_fingerprint", "source_snapshot_id"}
    }
    if content_digest(DigestKind.CATALOG, projection).hex != catalog["catalog_fingerprint"]:
        raise DiscoveryError(
            "catalog", "Captured catalogue fingerprint does not match its contents"
        )
    root = _directory(request["capture_root"])
    index = ModuleIndex.build(
        tuple(request["source_roots"]), tuple(f["path"] for f in request["files"])
    )
    if any(e.name in sys.modules for e in index.modules):
        raise DiscoveryError(
            "fresh_worker", "Discovery requires a fresh interpreter without source imports"
        )
    sources: dict[str, bytes] = {}
    total = 0
    for file in request["files"]:
        data = _read(root / file["path"], MAX_METADATA)
        total += len(data)
        if total > MAX_SOURCE_TOTAL:
            raise DiscoveryError(
                "limit", "Discovery source bytes exceed the configured service limit"
            )
        if (
            len(data) != int(file["byte_length"])
            or hashlib.sha256(data).hexdigest() != file["sha256"]
        ):
            raise DiscoveryError(
                "source_changed", "Captured source bytes no longer match the manifest", file["path"]
            )
        sources[file["path"]] = data
    codes: dict[str, CodeType] = {}
    for entry in index.modules:
        if entry.kind != "namespace":
            try:
                codes[entry.name] = compile(
                    sources[entry.path], str(root / entry.path), "exec", dont_inherit=True
                )
            except SyntaxError as exc:
                raise DiscoveryError(
                    "syntax",
                    "Captured Python source has a syntax error",
                    entry.path,
                    exc.lineno,
                    "SyntaxError",
                ) from exc
    snapshot = CatalogSnapshot(catalog)
    finder = CapturedFinder(root, index, codes)
    _bind_worker(snapshot, expected_fingerprint=catalog["catalog_fingerprint"])
    sys.meta_path.insert(0, finder)
    original_path = sys.path[:]
    sys.path[:0] = [str(root / path) for path in request["source_roots"]]
    definitions: list[dict[str, object]] = []
    imported: list[str] = []
    try:
        for entry in index.modules:
            try:
                module = importlib.import_module(entry.name)
                seen: set[int] = set()
                local: list[Declaration] = []
                for function in vars(module).values():
                    if not inspect.isfunction(function) or id(function) in seen:
                        continue
                    seen.add(id(function))
                    declaration = get_declaration(function)
                    if declaration is not None and declaration.module == entry.name:
                        local.append(declaration)
                if len(local) > 1:
                    raise DiscoveryError(
                        "multiple_producers",
                        "A producing module must define exactly one producer",
                        entry.path,
                    )
                definitions.extend(_declaration(d, entry) for d in local)
                imported.append(entry.name)
            except DiscoveryError:
                raise
            except BaseException as exc:
                # SystemExit/KeyboardInterrupt from authored imports are failures, too.
                frames = list(traceback.walk_tb(exc.__traceback__))
                line = next(
                    (
                        line
                        for f, line in reversed(frames)
                        if f.f_code.co_filename == str(root / entry.path)
                    ),
                    None,
                )
                raise DiscoveryError(
                    "import",
                    "Captured module import failed; inspect the named source and its dependencies",
                    entry.path,
                    line,
                    type(exc).__name__,
                ) from exc
        result = {
            "format_version": 1,
            "source_snapshot_id": catalog["source_snapshot_id"],
            "catalog_fingerprint": catalog["catalog_fingerprint"],
            "environment_fingerprint": request["environment_fingerprint"],
            "definitions": definitions,
            "imported_modules": imported,
            "module_index": {e.name: e.path for e in index.modules},
        }
        # JSON serialization normalizes dataclass tuple carriers before schema validation.
        result = json.loads(json.dumps(result, allow_nan=False))
        validate_document("DiscoveryResultV1", result)
        return cast(dict[str, object], result)
    finally:
        sys.path[:] = original_path
        sys.meta_path.remove(finder)


def _write(path: Path, value: object) -> str:
    data = json.dumps(value, allow_nan=False, sort_keys=True).encode()
    if len(data) > MAX_METADATA:
        raise DiscoveryError("limit", "Discovery metadata exceeds its size limit")
    with path.open("xb") as stream:
        os.chmod(path, 0o600)
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())
    return hashlib.sha256(data).hexdigest()


def serve(request_path: Path, socket_path: Path) -> int:
    """Private discovery worker command; caller owns process deadlines and bounded log drains."""
    request = _decode(_read(request_path, 1024 * 1024, private=True))
    validate_document("DiscoveryRequestV1", request)
    check_compatibility(
        protocol_major=request["protocol"]["major"], protocol_minor=request["protocol"]["minor"]
    )
    directory = _directory(request["result_directory"], private=True)
    _directory(str(socket_path.parent), private=True)
    if socket_path.is_symlink() or not stat.S_ISSOCK(socket_path.stat().st_mode):
        raise DiscoveryError("unsafe_path", "Discovery requires a private coordinator socket")
    with socket.socket(socket.AF_UNIX) as channel:
        channel.settimeout(10)
        channel.connect(str(socket_path))
        # Mutual possession of the private request nonce is proven before imports.
        # The coordinator never reveals the nonce to an unauthenticated peer.
        expected = bytes.fromhex(request["auth_token"])
        channel.sendall(expected)
        received = bytearray()
        while len(received) < len(expected):
            block = channel.recv(len(expected) - len(received))
            if not block:
                raise DiscoveryError("authentication", "Discovery coordinator handshake failed")
            received.extend(block)
        if bytes(received) != expected:
            raise DiscoveryError("authentication", "Discovery coordinator handshake failed")
        channel.settimeout(None)
        with channel.makefile("wb", buffering=0) as stream:
            return _session(request, directory, cast(BinaryIO, stream))


def _session(request: dict[str, Any], directory: Path, stream: BinaryIO) -> int:
    sequence = 0
    lock = threading.Lock()
    stopped = threading.Event()

    def send(message: dict[str, object]) -> None:
        nonlocal sequence
        with lock:
            frame = ControlFrame.from_json(
                {
                    "protocol": {"major": 1, "minor": 0},
                    "request_id": request["request_id"],
                    "attempt_id": request["attempt_id"],
                    "sequence": str(sequence),
                    "message": message,
                    "required_capabilities": ["discovery.v1"],
                    "extensions": [],
                }
            )
            write_frame(stream, frame)
            sequence += 1

    def heartbeat() -> None:
        while not stopped.wait(5):
            try:
                send({"type": "heartbeat"})
            except (ProtocolError, OSError):
                os._exit(1)  # Coordinator loss cannot authorize continued import work.

    send({"type": "hello", "operation": "discover", "capabilities": ["discovery.v1"]})
    thread = threading.Thread(target=heartbeat, daemon=True)
    thread.start()
    try:
        send({"type": "phase", "phase": "discovering"})
        try:
            result = collect(request)
        except ModuleIndexError as exc:
            raise DiscoveryError(
                "module_index", str(exc), exc.paths[0] if exc.paths else None
            ) from exc
        digest = _write(directory / "discovery.json", result)
        stopped.set()
        thread.join()
        send({"type": "discovery_ready", "result_path": "discovery.json", "result_digest": digest})
        send({"type": "completed"})
        return 0
    except Exception as exc:
        error = (
            exc
            if isinstance(exc, DiscoveryError)
            else DiscoveryError("discovery", "Discovery failed before a valid result was produced")
        )
        validate_document("DiscoveryDiagnosticV1", error.diagnostic)
        _write(directory / "discovery-error.json", error.diagnostic)
        stopped.set()
        thread.join()
        send(
            {
                "type": "error",
                "code": error.diagnostic["code"],
                "message": str(error),
                "retryable": False,
            }
        )
        return 1
    finally:
        stopped.set()
        thread.join()
