"""Real PTY CLI presentation checks against an offline managed Polars workspace."""

from __future__ import annotations

import errno
import fcntl
import json
import os
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(REPO / "python/sdk/src"), str(REPO / "python/worker/src")]
from transflow_worker.environment import lock_environment, sync_environment  # noqa: E402

# This is a deliberately slow synthetic producer, not synchronization in the verifier.
SOURCE = """import time
import polars as pl
from transflow import Input, Output, transform

@transform(
    output=Output("raw/items"),
)
def items():
    time.sleep(0.4)
    return pl.DataFrame({"id": [1, 2, 3]}).lazy()

@transform(
    output=Output("curated/left"),
    rows=Input("raw/items"),
)
def left(rows):
    time.sleep(0.6)
    return rows

@transform(
    output=Output("curated/right"),
    rows=Input("raw/items"),
)
def right(rows):
    time.sleep(0.6)
    return rows

@transform(
    output=Output("curated/end"),
    left=Input("curated/left"),
    right=Input("curated/right"),
)
def end(left, right):
    return pl.concat([left, right])

@transform(
    output=Output("curated/failure"),
    rows=Input("raw/items"),
)
def failure(rows):
    raise RuntimeError("Synthetic terminal failure")

TRANSFORMS = [items, left, right, end, failure]
"""
ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")


def terminal(
    command: list[str],
    root: Path,
    *,
    extra: dict[str, str] | None = None,
    cancel: bool = False,
    resize: bool = False,
) -> tuple[int, str]:
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
    environment = dict(os.environ)
    for key in ("NO_COLOR", "CI"):
        environment.pop(key, None)
    environment.update({"TERM": "xterm-256color", "LC_ALL": "en_US.UTF-8", **(extra or {})})
    chunks = bytearray()
    interrupted = False
    resized = False
    try:
        with subprocess.Popen(
            command,
            cwd=root,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=slave,
            stderr=slave,
            start_new_session=True,
        ) as process:
            os.close(slave)
            slave = -1
            deadline = time.monotonic() + 90
            try:
                while time.monotonic() < deadline:
                    if select.select([master], [], [], 0.1)[0]:
                        try:
                            data = os.read(master, 65536)
                        except OSError as error:
                            if error.errno != errno.EIO:
                                raise
                            break
                        if not data:
                            break
                        chunks.extend(data)
                        assert len(chunks) < 2 * 1024 * 1024, "Unbounded terminal output"
                    # React to an observed producer phase; never sleep and guess its state.
                    if b"] Running" in chunks:
                        if cancel and not interrupted:
                            process.send_signal(signal.SIGINT)
                            interrupted = True
                        if resize and not resized:
                            fcntl.ioctl(
                                master, termios.TIOCSWINSZ, struct.pack("HHHH", 8, 55, 0, 0)
                            )
                            resized = True
                    if process.poll() is not None and not select.select([master], [], [], 0)[0]:
                        break
                else:
                    raise AssertionError("Terminal command exceeded its deadline")
                status = process.wait(timeout=10)
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=10)
        assert not cancel or interrupted, chunks.decode(errors="replace")
        assert not resize or resized, chunks.decode(errors="replace")
        return status, chunks.decode("utf-8").replace("\r\n", "\n")
    finally:
        os.close(master)
        if slave >= 0:
            os.close(slave)


def main() -> None:
    cli, wheel, wheelhouse = [Path(v).resolve(strict=True) for v in sys.argv[1:4]]
    cases: list[str] = []
    with tempfile.TemporaryDirectory(prefix="tf-terminal-") as temp:
        root = Path(temp).resolve() / "workspace"
        subprocess.run([cli, "init", root], check=True, capture_output=True)
        (root / "requirements.in").write_text("polars==1.44.2\nduckdb==1.5.5\n")
        lock_environment(
            root,
            "requirements.in",
            "requirements.lock",
            "3.14",
            wheelhouse=wheelhouse,
            offline=True,
        )
        sync_environment(
            root,
            "requirements.in",
            "requirements.lock",
            "3.14",
            wheel,
            "0.0.0.dev0",
            wheelhouse=wheelhouse,
            offline=True,
        )
        with (root / "workspace.toml").open("a") as stream:
            stream.write("\n[execution]\nmax_jobs=2\ncpu_tokens=2\n")
        (root / "src/pipeline.py").write_text(SOURCE)
        base = [str(cli), "--workspace", str(root)]
        build = [*base, "build", "curated/end", "--mode", "full", "--python", sys.executable]
        status, output = terminal([*build, "--force"], root)
        assert status == 0, output
        assert "◐" in output and "◓" in output and "] Running" in output
        assert "\x1b[32m✓ curated/left" in output and "\x1b[32m✓ curated/right" in output
        assert "4/4 complete" in output and "Branch master" in output
        assert "\x1b[?25h" in output
        # Both independent jobs appear in the same running frame.
        assert any(
            "curated/left" in f and "curated/right" in f and f.count("] Running") >= 2
            for f in output.split("\x1b[?25l")
        ), output
        cases.append("parallel live phases and green completion")
        status, cached = terminal(build, root)
        assert status == 0 and "Cached" in cached, cached
        cases.append("cache hits")
        status, failed = terminal(
            [*base, "build", "curated/failure", "--python", sys.executable], root
        )
        assert status == 1 and "\x1b[31m✗ curated/failure" in failed, failed
        assert "\x1b[?25h" in failed
        cases.append("failed producer and cursor restoration")
        status, canceled = terminal([*build, "--force"], root, cancel=True)
        assert status == 130 and "Canceled" in canceled and "\x1b[?25h" in canceled, canceled
        cases.append("Ctrl-C and cursor restoration")
        status, resized = terminal([*build, "--force"], root, resize=True)
        assert status == 0 and "4/4 complete" in resized, resized
        cases.append("terminal resize")
        status, uncoloured = terminal(build, root, extra={"NO_COLOR": "1"})
        assert status == 0 and not re.search(
            r"\x1b\[[0-9;]+m", uncoloured.replace("\x1b[0m", "")
        ), uncoloured
        cases.append("NO_COLOR retains text status")
        status, ascii_output = terminal([*build, "--color", "never"], root, extra={"LC_ALL": "C"})
        assert status == 0 and "+ Cached" in ascii_output and "✓" not in ascii_output, ascii_output
        assert not re.search(r"\x1b\[[1-9][0-9;]*m", ascii_output), ascii_output
        cases.append("explicit colour disable and ASCII indicators")
        status, ci = terminal(build, root, extra={"CI": "true"})
        assert status == 0 and "\x1b[?25l" not in ci, ci
        cases.append("CI avoids cursor redraw")

        status, dumb = terminal(build, root, extra={"TERM": "dumb"})
        assert status == 0 and "\x1b" not in dumb, dumb
        cases.append("dumb terminal append-only progress")
        status, machine = terminal([*build, "--json", "--color", "always"], root)
        assert status == 0 and "\x1b" not in machine, machine
        assert json.loads(machine)["result"]["data"]["state"] == "SUCCEEDED"
        piped = subprocess.run(build, cwd=root, capture_output=True, check=True)
        assert b"\x1b" not in piped.stdout + piped.stderr
        cases.append("JSON and redirected output")
        for arguments, heading in [
            (["catalog", "list"], "Catalogue"),
            (["branch", "list"], "Data branches"),
            (["build", "list"], "Build history"),
            (["schedule", "list"], "Schedules"),
        ]:
            status, listing = terminal([*base, *arguments], root)
            assert status == 0 and heading in ANSI.sub("", listing), listing
        cases.append("human command tables")
        # The client attached to a persistent coordinator uses the same display.
        sys.path.insert(0, str(REPO / "python/tools"))
        from api_client import coordinator

        with coordinator(root, cli):
            status, attached = terminal([*build, "--force"], root)
            assert (
                status == 0 and "4/4 complete" in attached and "\x1b[32m✓ curated/end" in attached
            ), attached
        cases.append("persistent coordinator client")
        print(
            json.dumps(
                {
                    "cases": cases,
                    "final_screen": ANSI.sub("", output).split("Build  ✓ Succeeded")[-1],
                },
                indent=2,
            )
        )


if __name__ == "__main__":
    main()
