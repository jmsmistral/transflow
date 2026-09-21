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

hello = {"type": "hello", "operation": "discover", "capabilities": ["discovery.v1"]}
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
    send({"type": "phase", "phase": "running" if mode == "wrong_phase" else "discovering"})
    if mode == "phase_regression":
        send({"type": "phase", "phase": "setup"})
    if mode in {"descendant", "quiet_descendant", "cancel"}:
        child = os.fork()
        if child == 0:
            if mode == "quiet_descendant":
                channel.close()
                os.close(1)
                os.close(2)
                def term_received(_signal, _frame):
                    (root / "term.time").write_text(str(time.monotonic()))
                signal.signal(signal.SIGTERM, term_received)
            else:
                signal.signal(signal.SIGTERM, signal.SIG_IGN)
            (root / "descendant.pid").write_text(str(os.getpid()))
            while True:
                time.sleep(0.05)
                (root / "pulse").write_text(str(time.monotonic()))
        while not (root / "descendant.pid").exists():
            time.sleep(0.005)
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
    if mode != "missing_result":
        send({"type": "discovery_ready", "result_path": "discovery.json", "result_digest": "a" * 64})
    if mode == "duplicate_result":
        send({"type": "discovery_ready", "result_path": "discovery.json", "result_digest": "a" * 64})
    send({"type": "completed"})
    if mode == "post_terminal":
        send({"type": "heartbeat"})
    if mode == "nonzero":
        sys.exit(3)
channel.close()
