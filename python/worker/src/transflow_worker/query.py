"""Authenticated supervised SQL operation; result bytes stay in bounded private files."""

from __future__ import annotations

import socket
import stat
import threading
from pathlib import Path
from typing import Any, BinaryIO, cast

from .canonical import DigestKind, content_digest
from .discovery import _directory, _read
from .scratchpad import execute
from .wire import ControlFrame, _decode, validate_document, write_frame


def serve(request_path: Path, socket_path: Path) -> int:
    request = _decode(_read(request_path, 1024 * 1024, private=True))
    validate_document("QueryExecutionRequestV1", request)
    _directory(str(socket_path.parent), private=True)
    if socket_path.is_symlink() or not stat.S_ISSOCK(socket_path.stat().st_mode):
        raise ValueError("Query requires a private coordinator socket")
    if request["request_id"] != request["query"]["request_id"]:
        raise ValueError("Query identity mismatch")
    with socket.socket(socket.AF_UNIX) as channel:
        channel.settimeout(10)
        channel.connect(str(socket_path))
        token = bytes.fromhex(request["auth_token"])
        channel.sendall(token)
        received = bytearray()
        while len(received) < len(token):
            block = channel.recv(len(token) - len(received))
            if not block:
                raise ValueError("Query handshake failed")
            received.extend(block)
        if bytes(received) != token:
            raise ValueError("Query handshake failed")
        channel.settimeout(None)
        with channel.makefile("wb", buffering=0) as stream:
            return _session(request, cast(BinaryIO, stream))


def _session(request: dict[str, Any], stream: BinaryIO) -> int:
    lock, stop = threading.Lock(), threading.Event()
    sequence = 0

    def send(message: dict[str, Any]) -> None:
        nonlocal sequence
        with lock:
            frame = ControlFrame.from_json(
                {
                    "protocol": {"major": 1, "minor": 0},
                    "request_id": request["request_id"],
                    "attempt_id": request["attempt_id"],
                    "sequence": str(sequence),
                    "required_capabilities": ["duckdb.query.v1"],
                    "extensions": [],
                    "message": message,
                }
            )
            write_frame(stream, frame)
            sequence += 1

    def heartbeat() -> None:
        while not stop.wait(1):
            try:
                send({"type": "heartbeat"})
            except OSError, ValueError:
                from .lifetime import disconnected

                disconnected()

    send({"type": "hello", "operation": "query_preview", "capabilities": ["duckdb.query.v1"]})
    thread = threading.Thread(target=heartbeat, daemon=True)
    thread.start()
    try:
        send({"type": "phase", "phase": "setup"})
        send({"type": "phase", "phase": "querying"})
        execute(request["query"], request["limits"])
        result = _decode(
            _read(Path(request["query"]["result_directory"]) / "result.json", 65536, private=True)
        )
        validate_document("ScratchpadResultV1", result)
        send(
            {
                "type": "query_results",
                "results_path": "result.json",
                "results_digest": content_digest(DigestKind.COMPUTE, result).hex,
            }
        )
        send({"type": "completed"})
        return 0
    except Exception:
        send(
            {
                "type": "error",
                "code": "query_failed",
                "message": "Query failed; check SQL, dependencies, input types and disk budget",
                "retryable": False,
            }
        )
        return 1
    finally:
        stop.set()
        thread.join()
