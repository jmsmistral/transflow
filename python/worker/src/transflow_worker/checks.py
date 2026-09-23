"""Private authenticated canonical-check operation; no producer or publication authority."""

from __future__ import annotations

import os
import socket
import stat
import threading
from pathlib import Path
from typing import Any, BinaryIO, cast

from .canonical import DigestKind, content_digest
from .check_adapter import CheckError, execute
from .compatibility import check_compatibility
from .discovery import _directory, _read, _write
from .wire import ControlFrame, ProtocolError, _decode, validate_document, write_frame


def serve(request_path: Path, socket_path: Path) -> int:
    request = _decode(_read(request_path, 1024 * 1024, private=True))
    validate_document("CheckEvaluationRequestV1", request)
    check_compatibility(
        protocol_major=request["protocol"]["major"], protocol_minor=request["protocol"]["minor"]
    )
    directory = _directory(request["result_directory"], private=True)
    _directory(str(socket_path.parent), private=True)
    if socket_path.is_symlink() or not stat.S_ISSOCK(socket_path.stat().st_mode):
        raise CheckError("unsafe_path", "Evaluation requires a private coordinator socket")
    with socket.socket(socket.AF_UNIX) as channel:
        channel.settimeout(10)
        channel.connect(str(socket_path))
        expected = bytes.fromhex(request["auth_token"])
        channel.sendall(expected)
        received = bytearray()
        while len(received) < len(expected):
            block = channel.recv(len(expected) - len(received))
            if not block:
                raise CheckError("authentication", "Evaluation coordinator handshake failed")
            received.extend(block)
        if bytes(received) != expected:
            raise CheckError("authentication", "Evaluation coordinator handshake failed")
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
                    "required_capabilities": [
                        "duckdb.checks.v1",
                        "expectation.ast.v1",
                        "expectation.core.v1",
                    ],
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

    send(
        {
            "type": "hello",
            "operation": "evaluate_checks",
            "capabilities": ["duckdb.checks.v1", "expectation.ast.v1", "expectation.core.v1"],
        }
    )
    thread = threading.Thread(target=heartbeat, daemon=True)
    thread.start()
    try:
        send({"type": "phase", "phase": "setup"})
        send(
            {
                "type": "phase",
                "phase": "validating_inputs"
                if request["phase"] == "input"
                else "validating_outputs",
            }
        )
        result = execute(request)
        validate_document("CheckAggregatesV1", result)
        _write(directory / "checks.json", result)
        stopped.set()
        thread.join()
        send(
            {
                "type": "check_results",
                "results_path": "checks.json",
                "results_digest": content_digest(DigestKind.COMPUTE, result).hex,
            }
        )
        send({"type": "completed"})
        return 0
    except BaseException:
        # Authored SystemExit/KeyboardInterrupt must not become successful execution.
        error = CheckError("check_evaluation", "Canonical evaluation failed; no check may pass")
        validate_document("DiscoveryDiagnosticV1", error.diagnostic)
        _write(directory / "checks-error.json", error.diagnostic)
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
