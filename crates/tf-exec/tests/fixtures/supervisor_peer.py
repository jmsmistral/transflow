"""Synthetic hostile/noisy protocol peer; not a shipped worker operation."""
import json
import os
import signal
import socket
import struct
import sys
import threading
import time
from pathlib import Path

request_path = Path(sys.argv[sys.argv.index("--request") + 1])
request = json.loads(request_path.read_bytes())
root = Path(request["result_directory"])
mode = request["capture_root"]
assert request_path.stat().st_mode & 0o777 == 0o600
assert request_path.parent.stat().st_mode & 0o777 == 0o700
(root / "leader.pid").write_text(str(os.getpid()))
if mode == "startup_crash":
    os.write(1, b"startup stdout\n")
    os.write(2, b"startup stderr\n")
    os._exit(9)
if mode == "startup_hang":
    time.sleep(60)
channel = socket.socket(socket.AF_UNIX)
channel.connect(sys.argv[sys.argv.index("--control-socket") + 1])
token = bytes.fromhex(request["auth_token"])
channel.sendall(bytes(32) if mode == "bad_auth" else token)
ack = bytearray()
while len(ack) < 32:
    chunk = channel.recv(32 - len(ack))
    if not chunk:
        sys.exit(2)
    ack.extend(chunk)
assert bytes(ack) == token
if mode == "no_hello":
    time.sleep(60)
sequence = 0

def send(message, **changes):
    global sequence
    frame = {"protocol": {"major": 1, "minor": 0},
             "request_id": request["request_id"], "attempt_id": request["attempt_id"],
             "sequence": str(sequence), "message": message,
             "required_capabilities": ["discovery.v1"], "extensions": []}
    frame.update(changes)
    payload = json.dumps(frame).encode()
    channel.sendall(struct.pack(">I", len(payload)) + payload)
    sequence += 1

operation = sys.argv[sys.argv.index("transflow_worker") + 1]
hello = {"type": "hello", "operation": operation, "capabilities": ["discovery.v1"]}
if mode == "wrong_session":
    send(hello, attempt_id="00000000-0000-4000-8000-000000000099")
elif mode == "wrong_major":
    send(hello, protocol={"major": 2, "minor": 0})
elif mode == "oversized":
    channel.sendall(struct.pack(">I", 1048577))
elif mode == "partial":
    channel.sendall(b"\x00\x00")
    time.sleep(60)
elif mode == "invalid":
    channel.sendall(struct.pack(">I", 1) + b"{")
else:
    send(hello)
    phase = "validating_inputs" if operation == "evaluate_checks" else "discovering"
    send({"type": "phase", "phase": "running" if mode == "wrong_phase" else phase})
    if mode == "budget":
        keys = ["POLARS_MAX_THREADS", "OMP_NUM_THREADS", "OPENBLAS_NUM_THREADS",
                "MKL_NUM_THREADS", "NUMEXPR_MAX_THREADS", "VECLIB_MAXIMUM_THREADS"]
        (root / "threads.json").write_text(json.dumps({key: os.environ[key] for key in keys}))
        os.write(1, b"budget stdout\n")
        os.write(2, b"budget stderr\n")
        (root / "ready").touch()
        while not (root / "release").exists():
            time.sleep(0.005)
    if mode == "phase_regression":
        send({"type": "phase", "phase": "setup"})
    if mode in {"descendant", "quiet_descendant", "quiet_stopped_descendant", "cancel"}:
        child = os.fork()
        if child == 0:
            if mode in {"quiet_descendant", "quiet_stopped_descendant"}:
                channel.close()
                os.close(1)
                os.close(2)
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
            if mode == "quiet_stopped_descendant":
                # Leader exit can HUP/CONT an orphaned stopped group. Keep this
                # fixture alive and stopped so only supervisor KILL ends it.
                signal.signal(signal.SIGHUP, signal.SIG_IGN)
                def stay_stopped(_signal, _frame):
                    os.kill(os.getpid(), signal.SIGSTOP)
                signal.signal(signal.SIGCONT, stay_stopped)
            (root / "descendant.pid").write_text(str(os.getpid()))
            if mode == "quiet_stopped_descendant":
                # Cannot emit heartbeats or handle signals while grace elapses.
                os.kill(os.getpid(), signal.SIGSTOP)
            while True:
                time.sleep(0.05)
                (root / "pulse").write_text(str(time.monotonic()))
        while not (root / "descendant.pid").exists():
            time.sleep(0.005)
        if mode == "quiet_stopped_descendant":
            # Acknowledge the actual stop before allowing leader completion.
            waited, status = os.waitpid(child, os.WUNTRACED)
            assert waited == child and os.WIFSTOPPED(status)
        if mode == "cancel":
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
            while True:
                os.write(1, b"cancel stdout\n")
                os.write(2, b"cancel stderr\n")
                time.sleep(0.01)
    if mode == "silent":
        time.sleep(0.15)
    if mode == "noisy":
        def flood(fd):
            for _ in range(512):
                os.write(fd, b"z" * 8192)
        threads = [threading.Thread(target=flood, args=(fd,)) for fd in [1, 2]]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
    if mode == "redact":
        os.write(1, b"prefix synthetic-")
        time.sleep(0.05)
        os.write(1, b"credential suffix\nAutho")
        time.sleep(0.05)
        os.write(1, b"rization: Bearer synthetic-hidden\nCookie: hidden\n")
        os.write(2, request["auth_token"].encode() + b" \xff\x00end\n")
    else:
        os.write(1, b"readable stdout\n")
        os.write(2, b"readable stderr\n")
    if mode == "crash":
        os.kill(os.getpid(), signal.SIGKILL)
    if mode == "worker_error":
        send({"type": "error", "code": "synthetic", "message": "synthetic worker failure", "retryable": False})
        sys.exit(1)
    if operation == "evaluate_checks":
        send({"type": "check_results", "results_path": "checks.json", "results_digest": "a" * 64})
    elif mode != "missing_result":
        send({"type": "discovery_ready", "result_path": "discovery.json", "result_digest": "a" * 64})
    if mode == "duplicate_result":
        send({"type": "discovery_ready", "result_path": "discovery.json", "result_digest": "a" * 64})
    send({"type": "completed"})
    if mode == "post_terminal":
        send({"type": "heartbeat"})
    if mode == "nonzero":
        sys.exit(3)
channel.close()
