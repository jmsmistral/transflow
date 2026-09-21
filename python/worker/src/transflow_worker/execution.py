"""Private authenticated execute operation; no catalogue or publication authority."""

from __future__ import annotations

import os
import socket
import stat
import threading
from pathlib import Path
from typing import Any, BinaryIO, cast

from .canonical import artifact_digest
from .compatibility import check_compatibility
from .discovery import DiscoveryError, _directory, _read, _write
from .polars_adapter import ExecutionError, execute
from .wire import ControlFrame, ProtocolError, _decode, validate_document, write_frame


def serve(request_path: Path, socket_path: Path) -> int:
    request = _decode(_read(request_path, 1024 * 1024, private=True))
    validate_document("PolarsExecutionRequestV1", request)
    check_compatibility(
        protocol_major=request["protocol"]["major"], protocol_minor=request["protocol"]["minor"]
    )
    directory = _directory(request["result_directory"], private=True)
    _directory(str(socket_path.parent), private=True)
    if socket_path.is_symlink() or not stat.S_ISSOCK(socket_path.stat().st_mode):
        raise ExecutionError("unsafe_path", "Execution requires a private coordinator socket")
    with socket.socket(socket.AF_UNIX) as channel:
        channel.settimeout(10)
        channel.connect(str(socket_path))
        expected = bytes.fromhex(request["auth_token"])
        channel.sendall(expected)
        received = bytearray()
        while len(received) < len(expected):
            block = channel.recv(len(expected) - len(received))
            if not block:
                raise ExecutionError("authentication", "Execution coordinator handshake failed")
            received.extend(block)
        if bytes(received) != expected:
            raise ExecutionError("authentication", "Execution coordinator handshake failed")
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
                    "required_capabilities": ["polars.execute.v1"],
                    "extensions": [],
                }
            )
            write_frame(stream, frame)
            sequence += 1

    def heartbeat() -> None:
        while not stopped.wait(5):
            try:
                send({"type": "heartbeat"})
            except ProtocolError, OSError:
                os._exit(1)

    send({"type": "hello", "operation": "execute", "capabilities": ["polars.execute.v1"]})
    thread = threading.Thread(target=heartbeat, daemon=True)
    thread.start()
    try:
        send({"type": "phase", "phase": "setup"})
        manifest = execute(request, send)
        stopped.set()
        thread.join()
        send(
            {
                "type": "artifact_ready",
                "manifest_path": "artifact.json",
                "artifact_digest": artifact_digest(manifest).hex,
            }
        )
        send({"type": "completed"})
        return 0
    except BaseException as exc:
        # Authored SystemExit/KeyboardInterrupt must not become successful execution.
        error = (
            exc
            if isinstance(exc, DiscoveryError)
            else ExecutionError(
                "execution",
                "Polars execution failed; inspect the captured producer and retained logs",
                request["producer"]["path"],
                request["producer"]["line"],
                type(exc).__name__,
            )
        )
        validate_document("DiscoveryDiagnosticV1", error.diagnostic)
        _write(directory / "execution-error.json", error.diagnostic)
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
