"""Small POSIX process-group experiment for fixed, bounded-output fixture children."""

import json
import os
import selectors
import signal
import subprocess
import sys
from pathlib import Path


class Managed:
    def __init__(self, directory: Path, mode: str, deadline):
        self.root = directory
        self.mode = mode
        self.deadline = deadline
        self.process = None
        self.escalated = False

    def __enter__(self):
        environment = {
            "PATH": os.defpath,
            "TMPDIR": str(self.root),
            "PYTHONNOUSERSITE": "1",
            "POLARS_MAX_THREADS": "1",
            "OMP_NUM_THREADS": "1",
            "OPENBLAS_NUM_THREADS": "1",
        }
        self.process = subprocess.Popen(
            [sys.executable, "-I", "-B", str(Path(__file__).with_name("worker.py")), self.mode],
            cwd=self.root,
            env=environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
            text=True,
        )
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(self.process.stdout, selectors.EVENT_READ)
                if not selector.select(timeout=20):
                    raise TimeoutError("Fixture did not become ready")
            line = self.process.stdout.readline(4096)
            self.ready = json.loads(line)
            if self.ready.get("ready") is not True:
                raise ValueError("Invalid fixture readiness message")
            return self
        except BaseException:
            self.stop(grace=0)
            raise

    def stop(self, grace=1):
        process = self.process
        if process is None:
            return
        # Only this directly owned, unreaped child session is eligible for signalling.
        if process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass  # The child exited between poll and signal; communicate still reaps it.
        try:
            self.stdout, self.stderr = process.communicate(timeout=grace)
        except subprocess.TimeoutExpired:
            self.escalated = True
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            self.stdout, self.stderr = process.communicate(timeout=5)
        return process.returncode

    def tick(self, now):
        if not self.deadline.expired(now):
            return None
        self.stop()
        diagnostic = self.deadline.diagnostic(now)
        diagnostic["logs"] = self.stderr
        return diagnostic

    def cancel(self):
        self.stop()
        return {"outcome": "CANCELED", "phase": self.deadline.phase}

    def __exit__(self, kind, value, traceback):
        self.stop()
