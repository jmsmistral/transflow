"""Real public CLI and crash/restart journeys against the prepared dispatch workspace."""

from __future__ import annotations

import json
import signal
import socket
import sqlite3
import struct
import subprocess
import time
from contextlib import closing
from pathlib import Path
from typing import Any


def exercise(cli: Path, root: Path, python: str, probe: Path) -> list[str]:
    cases: list[str] = []

    def invoke(*args: str, code: int = 0) -> dict[str, Any]:
        p = subprocess.run(
            [cli, "--workspace", root, *args, "--json"],
            text=True,
            capture_output=True,
            timeout=180,
            check=False,
        )
        assert p.returncode == code, (args, p.returncode, p.stdout, p.stderr)
        value: dict[str, Any] = json.loads(p.stdout)
        assert value["exit_status"] == code, value
        return value

    def rows(sql: str, *args: object) -> list[tuple[Any, ...]]:
        with closing(sqlite3.connect(root / ".transflow/runtime/catalog.sqlite")) as db:
            return db.execute(sql, args).fetchall()

    def build(*args: str, code: int = 0) -> dict[str, Any]:
        return invoke("build", *args, code=code)["result"]["data"]  # type: ignore[no-any-return]

    (root / "src/public_new.py").write_text(
        "import polars as pl\nfrom transflow import transform, Output\n"
        '@transform(output=Output("new/direct"))\n'
        'def direct(): return pl.DataFrame({"id": [1]}).lazy()\n'
    )
    fresh = build("new/direct", "--python", python, "--branch", "public")
    assert fresh["state"] == "SUCCEEDED" and len(fresh["jobs"]) == 1
    cases.append(
        "one public build registers and publishes a newly declared output without manual sync"
    )
    first = build("curated/end", "--python", python, "--branch", "public")
    assert first["state"] == "SUCCEEDED" and len(first["jobs"]) == 4
    assert all(j["attempts"] for j in first["jobs"])
    assert build("show", first["id"])["id"] == first["id"]
    page = build("list", "--limit", "2")
    assert len(page["builds"]) == 2 and page["next_cursor"]
    assert build("list", "--cursor", page["next_cursor"])["builds"]
    assert build("logs", first["id"])["id"] == first["id"]
    cases.append("public build performs implicit discovery, checks and publication with history")
    plan = invoke("plan", "curated/end", "--python", python, "--branch", "public")
    plan_id = plan["result"]["plan_id"]
    accepted = build("--plan", plan_id)
    assert accepted["state"] == "SUCCEEDED"
    assert all(j["state"] == "CACHED" and not j["attempts"] for j in accepted["jobs"])
    invoke("build", "--plan", plan_id, code=1)
    invoke("build", "--plan", plan_id, "--force", code=2)
    cases.append("saved draft executes exactly once; overrides and silent replanning are refused")
    before = rows("SELECT count(*) FROM build_plans")
    invoke("build", "curated/end", "--no-wait", code=1)
    assert rows("SELECT count(*) FROM build_plans") == before
    cases.append("no-wait without persistent coordinator refuses before plan mutation")

    def queue(branch: str) -> str:
        result = subprocess.run(
            [
                probe,
                root,
                python,
                "full",
                "raw/items",
                json.dumps({"accept_only": True, "branch": branch}),
            ],
            text=True,
            capture_output=True,
            check=True,
            timeout=180,
        )
        value = json.loads(result.stdout)
        assert value["ok"], value
        return str(value["result"]["build"])

    canceled_queue = queue("cancel-queued")
    assert build("cancel", canceled_queue)["disposition"] == "Requested"
    canceled_report = build("show", canceled_queue)
    assert canceled_report["state"] == "CANCELED"
    assert all(not j["attempts"] for j in canceled_report["jobs"])
    resumed = queue("resume-queued")
    build("raw/items", "--python", python, "--branch", "public")
    assert build("show", resumed)["state"] == "SUCCEEDED"
    cases.append(
        "restart resumes untouched accepted queues; offline cancellation starts no producer"
    )

    # Find the input source version by its public catalogue identity.
    historical = next(j for j in first["jobs"] if len(j["inputs"]) == 1)["inputs"][0]["version"]
    selected = build(
        "curated/left",
        "--python",
        python,
        "--branch",
        "public",
        "--mode",
        "selected",
        "--force",
        "--pin",
        f"curated/left#rows={historical}",
    )
    exact_replay = build("replay", selected["id"], "--branch", "pinned-replay")
    assert exact_replay["jobs"][0]["inputs"][0]["version"] == historical
    assert len(exact_replay["jobs"]) == 1
    cases.append("replay keeps the historical boundary version and selected scope")

    items = root / "src/items.py"
    original = items.read_text()
    items.write_text("this is deliberately invalid Python\n")
    replay = build("replay", first["id"], "--branch", "replayed")
    assert replay["state"] == "SUCCEEDED" and replay["source"] == first["source"]
    assert items.read_text() == "this is deliberately invalid Python\n"
    assert replay["plan"]["context"]["replay_of"] == first["id"]
    items.write_text(original)
    cases.append("replay uses retained original source and scope despite an invalid later checkout")

    stale = invoke("plan", "raw/items", "--python", python, "--branch", "public")["result"][
        "plan_id"
    ]
    items.write_text(original + "\n# changed after planning\n")
    invoke("build", "--plan", stale, code=1)
    items.write_text(original)
    cases.append("changed checkout invalidates a saved draft instead of silently replanning")

    log = next((root / ".transflow/runtime/attempts").glob("*/*/stdout.log"))
    content = log.read_bytes()
    log.unlink()
    log.symlink_to(root / "workspace.toml")
    # The tampered file may belong to any prior build; identify its owning build.
    log_build = rows(
        "SELECT j.build_id FROM attempts a JOIN jobs j ON j.id=a.job_id WHERE a.id=?",
        log.parent.parent.name,
    )[0][0]
    invoke("build", "logs", log_build, code=1)
    log.unlink()
    log.write_bytes(content)
    cases.append("log inspection refuses symlink substitution")

    warn_code = original.replace(
        'Output("raw/items")',
        'Output("raw/items", checks=[Check(E.col("id").gte(99), "public_warn", on_error="WARN")])',
    )
    items.write_text(warn_code)
    human = subprocess.run(
        [cli, "--workspace", root, "build", "raw/items", "--python", python, "--branch", "public"],
        text=True,
        capture_output=True,
        check=True,
        timeout=180,
    )
    assert "Warning: check" in human.stdout
    cached_warn = build("raw/items", "--python", python, "--branch", "public")
    assert cached_warn["jobs"][0]["state"] == "CACHED"
    checks = cached_warn["jobs"][0]["cached_from"]["checks"]
    assert checks and checks[0]["outcome"] == "VIOLATION" and checks[0]["finished_us"]
    last_good = cached_warn["jobs"][0]["version"]
    items.write_text(warn_code.replace('on_error="WARN"', 'on_error="FAIL"'))
    failed = build("raw/items", "--python", python, "--branch", "public", code=1)
    assert failed["state"] == "FAILED"
    assert failed["jobs"][0]["attempts"][0]["checks"][0]["outcome"] == "VIOLATION"
    assert rows("SELECT count(*) FROM dataset_heads WHERE version_id=?", last_good) == [(1,)]
    items.write_text(original)
    cases.append(
        "public WARN and cached original checks stay visible; "
        "FAIL reports retain the last good head"
    )

    marker = root / "worker-entered"
    descendant = root / "worker-descendant"
    slow = original.replace(
        "def items():",
        f"def items():\n    from pathlib import Path\n    import time, subprocess\n"
        f"    child = subprocess.Popen(['/bin/sleep', '60'])\n"
        f"    Path({str(descendant)!r}).write_text(str(child.pid))\n"
        f'    Path({str(marker)!r}).write_text("started")\n    time.sleep(60)',
    )

    def await_marker(process: subprocess.Popen[str]) -> None:
        deadline = time.monotonic() + 30
        while not marker.exists():
            assert process.poll() is None, process.communicate()
            assert time.monotonic() < deadline, "worker did not start"
            time.sleep(0.02)

    def descendant_stopped() -> None:
        pid = descendant.read_text()
        deadline = time.monotonic() + 10
        while True:
            result = subprocess.run(
                ["ps", "-o", "stat=", "-p", pid], text=True, capture_output=True, check=False
            )
            if not result.stdout.strip() or result.stdout.strip().startswith("Z"):
                return
            assert time.monotonic() < deadline, "worker descendant remained alive"
            time.sleep(0.02)

    items.write_text(slow)
    command = [
        str(cli),
        "--workspace",
        str(root),
        "build",
        "raw/items",
        "--branch",
        "public",
        "--python",
        python,
        "--force",
        "--json",
    ]
    process = subprocess.Popen(command, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        await_marker(process)
        active = rows("SELECT id FROM builds WHERE state='RUNNING'")[0][0]
        process.send_signal(signal.SIGINT)
        stdout, stderr = process.communicate(timeout=20)
        assert process.returncode == 130, (stdout, stderr)
        descendant_stopped()
        assert json.loads(stdout)["result"]["data"]["state"] == "CANCELED"
        assert rows("SELECT count(*) FROM write_reservations WHERE build_id=?", active) == [(0,)]
        cases.append("Ctrl-C cancels only the waiting build and waits for durable cleanup")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=20)
    marker.unlink()
    process = subprocess.Popen(command, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        await_marker(process)
        interrupted = rows("SELECT id FROM builds WHERE state='RUNNING'")[0][0]
        process.kill()
        process.communicate(timeout=20)
        items.write_text(original)
        restarted = build("raw/items", "--python", python, "--branch", "public", "--force")
        assert restarted["state"] == "SUCCEEDED"
        descendant_stopped()
        assert rows("SELECT state FROM builds WHERE id=?", interrupted) == [("INTERRUPTED",)]
        assert rows("SELECT count(*) FROM write_reservations") == [(0,)]
        assert not list((root / ".transflow/runtime/attempts").glob("*/*/recovery.json"))
        cases.append(
            "real coordinator kill fences interrupted work and authenticates orphan cleanup"
        )
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=20)
    marker.unlink()
    server = subprocess.Popen(
        [cli, "--workspace", root, "serve", "--json"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        deadline = time.monotonic() + 30
        while True:
            assert server.poll() is None, server.communicate()
            registration = json.loads((root / ".transflow/runtime/runtime.json").read_text())
            if registration["pid"] == server.pid:
                break
            assert time.monotonic() < deadline, "coordinator did not register"
            time.sleep(0.02)
        host, port = registration["endpoint"].rsplit(":", 1)
        with socket.create_connection((host, int(port)), timeout=2) as channel:
            request = json.dumps(
                {
                    "version": 1,
                    "session": registration["session"],
                    "token": "invalid",
                    "operation": "cancel",
                    "build": first["id"],
                }
            ).encode()
            channel.sendall(struct.pack(">I", len(request)) + request)
            header = channel.recv(4)
            assert len(header) == 4
            size = struct.unpack(">I", header)[0]
            received = bytearray()
            while len(received) < size:
                part = channel.recv(size - len(received))
                assert part
                received.extend(part)
            assert json.loads(received)["ok"] is False
        assert build("show", first["id"])["state"] == "SUCCEEDED"
        cases.append("persistent control rejects an invalid capability before mutation")
        items.write_text(slow)
        submitted = build(
            "raw/items", "--python", python, "--branch", "public", "--force", "--no-wait"
        )
        assert submitted["state"] == "ACCEPTED"
        await_marker(server)
        canceled = build("cancel", submitted["id"])
        assert canceled["disposition"] in ("Requested", "AlreadyRequested")
        deadline = time.monotonic() + 20
        while build("show", submitted["id"])["state"] in ("QUEUED", "RUNNING"):
            assert time.monotonic() < deadline
            time.sleep(0.02)
        assert build("show", submitted["id"])["state"] == "CANCELED"
        assert build("cancel", submitted["id"])["disposition"] == "TooLate"
        items.write_text(original)
        completed = build("raw/items", "--python", python, "--branch", "public", "--force")
        assert completed["state"] == "SUCCEEDED"
        assert build("replay", completed["id"], "--branch", "server-replay")["state"] == "SUCCEEDED"
        cases.append(
            "persistent no-wait owns work after client exit; authenticated cancel and wait work"
        )
    finally:
        if server.poll() is None:
            server.send_signal(signal.SIGINT)
            stdout, stderr = server.communicate(timeout=20)
            assert server.returncode == 0, (stdout, stderr)
        items.write_text(original)
    config_path = root / "workspace.toml"
    original_config = config_path.read_text()
    config_path.write_text(
        original_config.replace(
            "[execution]", '[execution]\nmax_attempts=3\nretryable_classes=["transient_io"]'
        )
    )
    items.write_text(
        original.replace(
            "def items():",
            "def items():\n    from transflow import TransientIOError\n"
            "    raise TransientIOError('temporary fixture')",
        )
    )
    process = subprocess.Popen(
        [
            cli,
            "--workspace",
            root,
            "build",
            "raw/items",
            "--branch",
            "retry-cancel",
            "--python",
            python,
            "--json",
        ],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        deadline = time.monotonic() + 30
        while True:
            pending = rows(
                "SELECT j.build_id FROM jobs j JOIN data_branches b ON b.id=j.branch_id "
                "WHERE b.name='retry-cancel' AND j.state='RETRY_WAIT'"
            )
            if pending:
                break
            assert process.poll() is None, process.communicate()
            assert time.monotonic() < deadline, "retry wait was not observed"
            time.sleep(0.01)
        retry_build = pending[0][0]
        history = rows(
            "SELECT a.id,a.state,a.finished_at_us FROM attempts a JOIN jobs j ON j.id=a.job_id "
            "WHERE j.build_id=? ORDER BY a.attempt_no",
            retry_build,
        )
        assert history and all(a[1] == "FAILED" for a in history)
        build("cancel", retry_build)
        stdout, stderr = process.communicate(timeout=20)
        assert process.returncode == 130, (stdout, stderr)
        after = rows(
            "SELECT a.id,a.state,a.finished_at_us FROM attempts a JOIN jobs j ON j.id=a.job_id "
            "WHERE j.build_id=? ORDER BY a.attempt_no",
            retry_build,
        )
        assert after[: len(history)] == history
        assert rows("SELECT count(*) FROM write_reservations WHERE build_id=?", retry_build) == [
            (0,)
        ]
        cases.append(
            "cancellation during retry backoff preserves completed failure history "
            "and releases reservations"
        )
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=20)
        items.write_text(original)
        config_path.write_text(original_config)
    return cases
