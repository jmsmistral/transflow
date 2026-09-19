"""Fresh installed workers, real sockets and captured imports; no producer invocation."""

import hashlib
import json
import os
import signal
import socket
import subprocess
import tempfile
from pathlib import Path
from typing import Any, BinaryIO, cast
from uuid import uuid4

import pytest
from test_packaging import installed as installed
from test_packaging import wheel as wheel
from transflow_worker.canonical import catalog_fingerprint
from transflow_worker.wire import Session, read_frame, validate_document


def request(
    root: Path, sources: dict[str, str], entries: list[dict[str, object]] | None = None
) -> dict[str, Any]:
    capture = root / "capture"
    capture.mkdir()
    files = []
    for path, text in sources.items():
        file = capture / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text)
        files.append(
            {
                "path": path,
                "sha256": hashlib.sha256(file.read_bytes()).hexdigest(),
                "byte_length": str(file.stat().st_size),
            }
        )
    catalog = {
        "format_version": 1,
        "workspace_id": str(uuid4()),
        "source_snapshot_id": str(uuid4()),
        "catalog_fingerprint": "0" * 64,
        "entries": entries or [],
    }
    catalog["catalog_fingerprint"] = catalog_fingerprint(catalog).hex
    return {
        "format_version": 1,
        "protocol": {"major": 1, "minor": 0},
        "request_id": str(uuid4()),
        "attempt_id": str(uuid4()),
        "capture_root": str(capture.resolve()),
        "source_roots": ["src"],
        "files": files,
        "catalog": catalog,
        "environment_fingerprint": "a" * 64,
        "result_directory": "filled_by_runner",
        "auth_token": os.urandom(32).hex(),
    }


def run_worker(
    python: Path, value: dict[str, Any], *, bad_auth: bool = False
) -> tuple[int, dict[str, Any], list[str]]:
    with tempfile.TemporaryDirectory(prefix="tf-discovery-", dir="/tmp") as short:
        directory = Path(short).resolve()
        value["result_directory"] = str(directory)
        request_file = directory / "request.json"
        request_file.write_text(json.dumps(value))
        request_file.chmod(0o600)
        control = directory / "control.sock"
        with socket.socket(socket.AF_UNIX) as listener:
            listener.bind(str(control))
            listener.listen(1)
            listener.settimeout(10)
            child = subprocess.Popen(
                [
                    str(python),
                    "-I",
                    "-B",
                    "-m",
                    "transflow_worker",
                    "discover",
                    "--request",
                    str(request_file),
                    "--control-socket",
                    str(control),
                ],
                cwd=directory,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                start_new_session=True,
                env={**os.environ, "PYTHONPATH": str(directory / "poison")},
            )
            messages = []
            try:
                channel, _ = listener.accept()
                with channel:
                    channel.settimeout(10)
                    channel.sendall(bytes(32) if bad_auth else bytes.fromhex(value["auth_token"]))
                    session = Session(
                        value["request_id"],
                        value["attempt_id"],
                        "discover",
                        frozenset({"discovery.v1"}),
                    )
                    with channel.makefile("rb") as stream:
                        while frame := read_frame(cast(BinaryIO, stream)):
                            session.accept(frame)
                            messages.append(frame.message_type)
                            if frame.message_type == "discovery_ready":
                                ready = frame.as_json()["message"]
                                data = (directory / ready["result_path"]).read_bytes()
                                assert hashlib.sha256(data).hexdigest() == ready["result_digest"]
                code = child.wait(timeout=10)
            finally:
                if child.poll() is None:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait(timeout=10)
            output = directory / ("discovery.json" if code == 0 else "discovery-error.json")
            result = json.loads(output.read_bytes()) if output.exists() else {}
            if result:
                validate_document(
                    "DiscoveryResultV1" if code == 0 else "DiscoveryDiagnosticV1", result
                )
            return code, result, messages


def test_forward_strings_helpers_reexports_once_and_no_producers(
    installed: Path, tmp_path: Path
) -> None:
    marker, called = tmp_path / "imports.txt", tmp_path / "producer.txt"
    sources = {
        "src/pkg/__init__.py": "from .source import source\n",
        "src/pkg/source.py": f"""from pathlib import Path
from transflow import source_transform, Output
with Path({str(marker)!r}).open('a') as out: out.write('once')
@source_transform(output=Output('raw/items'))
def source():
    Path({str(called)!r}).touch()
""",
        "src/aaa.py": """from transflow import transform, Input, Output
from pkg import source
@transform(items=Input('raw/items'), output=Output('curated/items'))
def derived(items): return items
again = derived
print('log output is separate from control')
""",
        "src/helpers.py": "def helper(x): return x + 1\n",
    }
    value = request(tmp_path, sources)
    code, result, messages = run_worker(installed, value)
    assert code == 0 and messages == ["hello", "phase", "discovery_ready", "completed"], (
        code,
        result,
        messages,
    )
    assert [d["module"] for d in result["definitions"]] == ["aaa", "pkg.source"]
    assert result["definitions"][0]["inputs"][0]["ref"] == {"form": "string", "value": "raw/items"}
    assert marker.read_text() == "once" and not called.exists()
    assert all(d["path"].startswith("src/") and d["line"] > 0 for d in result["definitions"])
    # Another interpreter sees the same logical declarations, not prior sys.modules.
    code2, result2, _ = run_worker(installed, value)
    assert code2 == 0 and result == result2
    assert marker.read_text() == "onceonce"


@pytest.mark.parametrize(
    "source,code",
    [
        ("raise RuntimeError('private raw exception must not escape')", "import"),
        ("raise SystemExit(0)", "import"),
        ("from transflow.catalog import C\nx = C.missing", "import"),
        ("def invalid(:", "syntax"),
        (
            "from transflow import transform, Output\n"
            "@transform(output=Output('a'))\ndef a(): pass\n"
            "@transform(output=Output('b'))\ndef b(): pass",
            "multiple_producers",
        ),
    ],
)
def test_import_failures_are_structured(
    installed: Path, tmp_path: Path, source: str, code: str
) -> None:
    status, result, messages = run_worker(installed, request(tmp_path, {"src/broken.py": source}))
    assert status == 1 and messages[-1] == "error", (status, result, messages)
    assert result["code"] == code and result["path"] == "src/broken.py", result
    assert "private raw exception" not in json.dumps(result)


def test_parse_all_sources_before_import_side_effects(installed: Path, tmp_path: Path) -> None:
    marker = tmp_path / "must-not-exist"
    value = request(
        tmp_path,
        {
            "src/aaa.py": f"from pathlib import Path\nPath({str(marker)!r}).touch()",
            "src/zzz.py": "def broken(:",
        },
    )
    assert run_worker(installed, value)[0] == 1
    assert not marker.exists()


def test_captured_c_owns_identity_and_policy(installed: Path, tmp_path: Path) -> None:
    owner, dataset = str(uuid4()), str(uuid4())
    entry: dict[str, object] = {
        "key": {"workspace_id": owner, "dataset_id": dataset},
        "path": "external/provider/items",
        "kind": "external",
    }
    value = request(
        tmp_path,
        {
            "src/model.py": """from transflow import transform, Input, Output, Branch
from transflow.catalog import C
@transform(items=Input(C.external.provider.items, branch=Branch.CURRENT,
                       stop_branch_fallback=True), output=Output('out'))
def model(items): return items
"""
        },
        [entry],
    )
    status, result, _ = run_worker(installed, value)
    assert status == 0
    binding = result["definitions"][0]["inputs"][0]
    assert binding["ref"]["workspace_id"] == owner and binding["ref"]["dataset_id"] == dataset
    assert binding["ref"]["catalog_fingerprint"] == value["catalog"]["catalog_fingerprint"]
    assert binding["branch"]["kind"] == "current" and binding["stop_branch_fallback"]


@pytest.mark.parametrize("mutation", ["hash", "catalog", "symlink", "shadow"])
def test_capture_and_index_guards(installed: Path, tmp_path: Path, mutation: str) -> None:
    value = request(tmp_path, {"src/helper.py": "pass"})
    file = Path(value["capture_root"]) / "src/helper.py"
    if mutation == "hash":
        file.write_text("raise Exception('changed')")
    elif mutation == "catalog":
        value["catalog"]["catalog_fingerprint"] = "f" * 64
    elif mutation == "symlink":
        other = tmp_path / "other.py"
        other.write_text("pass")
        file.unlink()
        file.symlink_to(other)
    else:
        renamed = file.with_name("transflow.py")
        file.rename(renamed)
        value["files"][0]["path"] = "src/transflow.py"
    code, result, _ = run_worker(installed, value)
    assert code == 1 and result["code"] in {
        "source_changed",
        "catalog",
        "unsafe_path",
        "module_index",
    }


def test_failed_authentication_precedes_imports(installed: Path, tmp_path: Path) -> None:
    marker = tmp_path / "not-imported"
    value = request(
        tmp_path, {"src/helper.py": f"from pathlib import Path\nPath({str(marker)!r}).touch()"}
    )
    status, _, frames = run_worker(installed, value, bad_auth=True)
    assert status == 1 and frames == [] and not marker.exists()


def test_check_resource_and_parameter_metadata(installed: Path, tmp_path: Path) -> None:
    value = request(
        tmp_path,
        {
            "src/model.py": """from transflow import transform, Input, Output, Check, Parameter
from transflow import expectations as E
@transform(items=Input('raw/items', checks=Check(E.primary_key('id'), 'Key', sample_rows=0)),
           output=Output('out'), resources={'wall_timeout_seconds': 0},
           params={'limit': Parameter({'type':'i64'}, {'type':'i64','value':'5'})})
def model(items): return items
"""
        },
    )
    code, result, _ = run_worker(installed, value)
    assert code == 0, result
    definition = result["definitions"][0]
    assert definition["wall_timeout_seconds"] == "0"
    assert definition["inputs"][0]["checks"][0]["sample_rows"] == "0"
    assert definition["parameters"][0]["default"] == {"type": "i64", "value": "5"}


def test_precompiled_bytes_ignore_changes_during_import(installed: Path, tmp_path: Path) -> None:
    value = request(tmp_path, {"src/bbb.py": "value = 1", "src/aaa.py": "pass"})
    helper = Path(value["capture_root"]) / "src/bbb.py"
    first = Path(value["capture_root"]) / "src/aaa.py"
    first.write_text(
        f"from pathlib import Path\nPath({str(helper)!r}).write_text('raise Exception()')\n"
        "import bbb\nassert bbb.value == 1\n"
    )
    for file in value["files"]:
        source = Path(value["capture_root"]) / file["path"]
        file.update(
            sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
            byte_length=str(source.stat().st_size),
        )
    assert run_worker(installed, value)[0] == 0


def test_worker_collects_explicit_alias_refs_and_rejects_alias_fingerprint_tampering(
    installed: Path, tmp_path: Path
) -> None:
    value = request(
        tmp_path,
        {
            "src/model.py": """from transflow import transform, Input, Output
from transflow.catalog import C
@transform(items=Input(C.external.alternate.items), output=Output(C.old.items))
def model(items): return items
"""
        },
    )
    local = {"workspace_id": value["catalog"]["workspace_id"], "dataset_id": str(uuid4())}
    foreign = {"workspace_id": str(uuid4()), "dataset_id": str(uuid4())}
    value["catalog"]["entries"] = [
        {"path": "raw/items", "key": local, "kind": "transform"},
        {"path": "external/provider/items", "key": foreign, "kind": "external"},
    ]
    value["catalog"]["aliases"] = [
        {"path": "old/items", "key": local},
        {"path": "external/alternate/items", "key": foreign},
    ]
    value["catalog"]["catalog_fingerprint"] = catalog_fingerprint(value["catalog"]).hex
    status, result, _ = run_worker(installed, value)
    assert status == 0
    definition = result["definitions"][0]
    assert definition["output"]["ref"]["path"] == "old/items"
    assert definition["output"]["ref"]["dataset_id"] == local["dataset_id"]
    assert definition["inputs"][0]["ref"]["workspace_id"] == foreign["workspace_id"]
    value["catalog"]["aliases"][0]["path"] = "old/changed"
    status, result, _ = run_worker(installed, value)
    assert status == 1 and result["code"] == "catalog"
