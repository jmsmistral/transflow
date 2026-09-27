"""Bounded real-coordinator client for public synthetic API qualification."""

from __future__ import annotations

import http.client
import json
import signal
import subprocess
import time
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any
from urllib.parse import urlencode
from uuid import uuid4


@contextmanager
def coordinator(root: Path, cli: Path) -> Iterator[dict[str, Any]]:
    with subprocess.Popen(
        [str(cli), "--workspace", str(root), "serve", "--json"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    ) as process:
        try:
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise AssertionError(process.communicate())
                try:
                    registration = json.loads(
                        (root / ".transflow/runtime/runtime.json").read_text()
                    )
                    if registration["pid"] == process.pid and registration.get("http_endpoint"):
                        yield registration
                        break
                except FileNotFoundError, json.JSONDecodeError:
                    pass
                time.sleep(0.02)
            else:
                raise AssertionError("HTTP coordinator did not become ready")
        finally:
            process.send_signal(signal.SIGINT)
            out, err = process.communicate(timeout=20)
            assert process.returncode == 0, (out, err)


class Client:
    def __init__(self, registration: dict[str, Any]):
        self.endpoint = registration["http_endpoint"]
        self.token = registration["nonce"]

    def call(
        self,
        method: str,
        path: str,
        body: object | None = None,
        *,
        query: dict[str, str] | None = None,
        headers: dict[str, str] | None = None,
        auth: bool = True,
        status: int | None = 200,
    ) -> dict[str, Any]:
        connection = http.client.HTTPConnection(self.endpoint, timeout=180)
        sent = {"Content-Type": "application/json"}
        if auth:
            sent["Authorization"] = f"Bearer {self.token}"
        sent.update(headers or {})
        try:
            connection.request(
                method,
                path + ("?" + urlencode(query) if query else ""),
                None if body is None else json.dumps(body),
                sent,
            )
            response = connection.getresponse()
            value = json.loads(response.read())
            assert status is None or response.status == status, (
                method,
                path,
                response.status,
                value,
            )
            assert response.getheader("X-Request-ID")
            return value  # type: ignore[no-any-return]
        finally:
            connection.close()

    def context(self, **extra: str) -> dict[str, Any]:
        return self.call("GET", "/api/v1/context", query={"branch": "main", **extra})

    def mutate(
        self,
        path: str,
        body: object,
        *,
        context: dict[str, Any] | None = None,
        key: str | None = None,
        status: int = 200,
        **query: str,
    ) -> dict[str, Any]:
        context = context or self.context(**query)
        return self.call(
            "POST",
            path,
            body,
            query={"branch": "main", **query},
            headers={
                "If-Match": f'"{context["context"]["fingerprint"]}"',
                "Idempotency-Key": key or str(uuid4()),
            },
            status=status,
        )
