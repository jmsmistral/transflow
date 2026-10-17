"""Actual framed native service, installed metadata and hostile/unfinished drafts."""

from __future__ import annotations

import json
import queue
import subprocess
import sys
import threading
import tomllib
from pathlib import Path
from typing import Any
from uuid import uuid4

import pytest
from test_catalog_cli import CLI, DECLARATION, cli, files, source
from test_catalog_cli import workspace as workspace
from test_packaging import wheel as wheel


class Client:
    def __init__(self, roots: list[Path]) -> None:
        self.process = subprocess.Popen(
            [str(CLI), "lsp", "--python", sys.executable],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.sequence = 0
        self.frames: queue.Queue[dict[str, Any] | BaseException] = queue.Queue()
        threading.Thread(target=self._read, daemon=True).start()
        result = self.call(
            "initialize",
            {"workspaceFolders": [{"uri": root.as_uri(), "name": root.name} for root in roots]},
        )
        assert result["capabilities"]["positionEncoding"] == "utf-16"
        self.send("initialized", {})

    def _read(self) -> None:
        assert self.process.stdout is not None
        try:
            while True:
                header = self.process.stdout.readline()
                if not header:
                    return
                assert header.startswith(b"Content-Length: "), header
                length = int(header.split(b":")[1])
                assert 0 < length <= 1024 * 1024
                assert self.process.stdout.readline() == b"\r\n"
                self.frames.put(json.loads(self.process.stdout.read(length)))
        except BaseException as error:
            self.frames.put(error)

    def send(self, method: str, params: dict[str, Any], identity: int | None = None) -> None:
        value: dict[str, Any] = {"jsonrpc": "2.0", "method": method, "params": params}
        if identity is not None:
            value["id"] = identity
        body = json.dumps(value).encode()
        assert self.process.stdin is not None
        self.process.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
        self.process.stdin.flush()

    def call(self, method: str, params: dict[str, Any]) -> Any:
        self.sequence += 1
        self.send(method, params, self.sequence)
        while True:
            value = self.frames.get(timeout=15)
            if isinstance(value, BaseException):
                raise value
            if value.get("id") == self.sequence:
                assert "error" not in value, value
                return value["result"]

    def open(self, path: Path, text: str, version: int = 1) -> dict[str, Any]:
        self.send(
            "textDocument/didOpen",
            {
                "textDocument": {
                    "uri": path.as_uri(),
                    "languageId": "python",
                    "version": version,
                    "text": text,
                }
            },
        )
        return {"textDocument": {"uri": path.as_uri()}}

    def close(self) -> None:
        self.call("shutdown", {})
        self.send("exit", {})
        assert self.process.wait(timeout=10) == 0
        assert self.process.stderr is not None
        assert self.process.stderr.read() == b""
        for pipe in (self.process.stdin, self.process.stdout, self.process.stderr):
            assert pipe is not None
            pipe.close()


def simple(root: Path, name: str) -> Path:
    cli(root, "init", str(root))
    registry = root / ".transflow/catalog.toml"
    registry.write_text(
        registry.read_text()
        + (f'\n[[datasets]]\nid="{uuid4()}"\npath="raw/{name}"\nkind="transform"\n')
    )
    return root


def labels(client: Client, doc: dict[str, Any], line: int, character: int) -> set[str]:
    result = client.call(
        "textDocument/completion",
        {
            **doc,
            "position": {"line": line, "character": character},
        },
    )
    return {item["label"] for item in result["items"]}


def test_isolated_catalogue_completion_refresh_and_invalid_draft(tmp_path: Path) -> None:
    a = simple(tmp_path / "orders", "orders")
    b = simple(tmp_path / "invoices", "invoices")
    before = files(a), files(b)
    client = Client([a, b])
    try:
        text = "from transflow.catalog import C\nC.raw."
        adoc = client.open(a / "src/é draft.py", text)
        bdoc = client.open(b / "src/invoices.py", text)
        assert labels(client, adoc, 1, 6) == {"orders"}
        assert labels(client, bdoc, 1, 6) == {"invoices"}
        assert any(
            d["code"] == "syntax" for d in client.call("textDocument/diagnostic", adoc)["items"]
        )
        client.open(a / "src/é draft.py", "from transflow.catalog import C\nC.raw.invoices", 2)
        diagnostics = client.call("textDocument/diagnostic", adoc)["items"]
        assert any(d["code"] == "unknown-dataset" for d in diagnostics)
        registry = a / ".transflow/catalog.toml"
        original = registry.read_text()
        registry.write_text(original.replace("raw/orders", "raw/shipments"))
        client.open(a / "src/é draft.py", text, 3)
        assert labels(client, adoc, 1, 6) == {"shipments"}
        assert labels(client, bdoc, 1, 6) == {"invoices"}
        registry.write_text("invalid TOML")
        assert labels(client, adoc, 1, 6) == {"shipments"}
        assert any(
            d["code"] == "stale-catalogue"
            for d in client.call("textDocument/diagnostic", adoc)["items"]
        )
        registry.write_text(original)
        assert (files(a), files(b)) == before
    finally:
        client.close()


def test_static_references_and_exact_string_replacement(tmp_path: Path) -> None:
    root = simple(tmp_path / "workspace", "orders")
    client = Client([root])
    try:
        doc = client.open(root / "src/draft.py", 'from transflow import Input\nInput("raw/or')
        result = client.call(
            "textDocument/completion",
            {
                **doc,
                "position": {"line": 1, "character": 13},
            },
        )
        item = result["items"][0]
        assert item["textEdit"]["newText"] == "raw/orders"
        assert item["textEdit"]["range"]["start"] == {"line": 1, "character": 7}
        middle = client.open(
            root / "src/middle.py", 'from transflow import Input\nInput("raw/orders")'
        )
        item = client.call(
            "textDocument/completion",
            {
                **middle,
                "position": {"line": 1, "character": 13},
            },
        )["items"][0]
        assert item["textEdit"]["range"]["end"] == {"line": 1, "character": 17}
        # Pure source data: neither imports nor top-level side effects execute.
        sentinel = root / "must-not-exist"
        text = (
            f"from pathlib import Path\nPath({str(sentinel)!r}).touch()\n"
            "from transflow.catalog import C\n# C.raw.typo\n"
            's="C.raw.typo"\nx=C.raw.orders\ny=C.raw.typo\n'
        )
        client.open(root / "src/draft.py", text, 2)
        diagnostics = client.call("textDocument/diagnostic", doc)["items"]
        unknown = [d for d in diagnostics if d["code"] == "unknown-dataset"]
        assert len(unknown) == 1 and unknown[0]["range"]["start"]["line"] == 6
        assert not sentinel.exists()
        hover = client.call(
            "textDocument/hover",
            {
                **doc,
                "position": {"line": 5, "character": 10},
            },
        )
        assert "working-tree metadata" in hover["contents"]["value"]
        assert "Built version/schema: unknown" in hover["contents"]["value"]
        assert "run catalog sync" in hover["contents"]["value"]
        # A local shadow is ordinary Python, outside the Transflow feature scope.
        client.open(root / "src/draft.py", "from transflow.catalog import C\nC=object()\nC.nope", 3)
        assert not [
            d
            for d in client.call("textDocument/diagnostic", doc)["items"]
            if d["code"] == "unknown-dataset"
        ]
    finally:
        client.close()


def test_retained_producer_navigation_without_reimport(workspace: Path) -> None:
    source(workspace, "orders.py", DECLARATION)
    cli(workspace, "catalog", "sync", "--python", sys.executable)
    source(workspace, "orders.py", 'raise AssertionError("Must not import")\n')
    # A read-only SQLite connection may create/rebuild its transient WAL index.
    before = {p: h for p, h in files(workspace).items() if not p.endswith(("-shm", "-wal"))}
    client = Client([workspace])
    try:
        doc = client.open(
            workspace / "src/draft.py", "from transflow.catalog import C\nC.raw.orders"
        )
        point = {**doc, "position": {"line": 1, "character": 8}}
        location = client.call("textDocument/definition", point)
        assert location["uri"] == (workspace / "src/orders.py").as_uri()
        # Discovery retains the declaration's decorator line (one-based 2).
        assert location["range"]["start"]["line"] == 1
        hover = client.call("textDocument/hover", point)["contents"]["value"]
        assert "Descriptor source snapshot:" in hover and "captured metadata" in hover
        outside = workspace.parent / "outside.py"
        outside.write_text("pass\n")
        (workspace / "src/orders.py").unlink()
        (workspace / "src/orders.py").symlink_to(outside)
        assert client.call("textDocument/definition", point) is None
        (workspace / "src/orders.py").unlink()
        source(workspace, "orders.py", 'raise AssertionError("Must not import")\n')
        assert {
            p: h for p, h in files(workspace).items() if not p.endswith(("-shm", "-wal"))
        } == before
    finally:
        client.close()


def test_foreign_identity_collision_never_opens_a_local_producer(
    workspace: Path,
    tmp_path: Path,
) -> None:
    source(workspace, "orders.py", DECLARATION)
    cli(workspace, "catalog", "sync", "--python", sys.executable)
    local = tomllib.loads((workspace / ".transflow/catalog.toml").read_text())["datasets"][0]["id"]
    provider = simple(tmp_path / "provider", "invoices")
    registry = provider / ".transflow/catalog.toml"
    text = registry.read_text()
    foreign = tomllib.loads(text)["datasets"][0]["id"]
    registry.write_text(text.replace(foreign, local))
    cli(
        workspace,
        "external",
        "add",
        "--workspace",
        str(provider),
        "--dataset",
        "raw/invoices",
        "--as",
        "provider.invoices",
    )
    client = Client([workspace])
    try:
        doc = client.open(
            workspace / "src/draft.py",
            "from transflow.catalog import C\nC.external.provider.invoices",
        )
        point = {**doc, "position": {"line": 1, "character": 15}}
        assert client.call("textDocument/definition", point) is None
        hover = client.call("textDocument/hover", point)["contents"]["value"]
        assert "external/provider/invoices" in hover
        assert "external provider not contacted" in hover
        assert "src/orders.py" not in hover
    finally:
        client.close()


@pytest.mark.parametrize("payload", [b"Content-Length: 10000000\r\n\r\n", b"bad\r\n\r\n"])
def test_protocol_failure_has_no_product_envelope(payload: bytes) -> None:
    result = subprocess.run([str(CLI), "lsp"], input=payload, capture_output=True, timeout=10)
    assert result.returncode == 1 and result.stdout == b""
    assert result.stderr == b"Invalid or excessive LSP input frame\n"
