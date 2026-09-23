"""Private emergency cleanup for a worker whose coordinator has disappeared."""

from __future__ import annotations

import hmac
import os
import signal
import socket
import threading
from pathlib import Path
from typing import NoReturn

_GROUP: int | None = None


def start(request_path: Path) -> None:
    """Expose only authenticated self-group termination, never an arbitrary PID API.

    The supervisor creates a private request before launch. Normal cancellation
    remains TERM/grace/KILL under the direct-child supervisor. Recovery is emergency
    KILL after the replacement coordinator fences the lost session.
    """
    if os.environ.pop("TRANSFLOW_RECOVERABLE_WORKER", "") != "1":
        return
    from .discovery import _read
    from .wire import _decode

    request = _decode(_read(request_path, 1024 * 1024, private=True))
    token = bytes.fromhex(request["auth_token"])
    if len(token) != 32 or os.getpgrp() != os.getpid():
        raise ValueError("Invalid recoverable worker identity")
    global _GROUP
    _GROUP = os.getpid()
    listener = socket.socket(socket.AF_UNIX)
    listener.bind(str(request_path.parent / "recovery.sock"))
    listener.listen(2)

    def serve() -> None:
        while True:
            channel, _ = listener.accept()
            with channel:
                channel.settimeout(1)
                try:
                    received = bytearray()
                    while len(received) < 32:
                        block = channel.recv(32 - len(received))
                        if not block:
                            break
                        received.extend(block)
                    if not hmac.compare_digest(token, received):
                        continue
                    channel.sendall(token)
                    if channel.recv(1) == b"K" and os.getpgrp() == os.getpid():
                        os.killpg(os.getpgrp(), signal.SIGKILL)
                except OSError:
                    continue

    threading.Thread(target=serve, name="transflow-recovery", daemon=True).start()


def disconnected() -> NoReturn:
    """A directly authenticated worker cleans its own descendants on coordinator loss."""
    if _GROUP == os.getpid() == os.getpgrp():
        os.killpg(_GROUP, signal.SIGKILL)
    os._exit(1)
