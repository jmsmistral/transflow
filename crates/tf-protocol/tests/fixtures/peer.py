"""Synthetic socket peer; no user code or engines are imported."""
import json
import socket
import sys

from transflow_worker.wire import ControlFrame, read_frame, write_frame

request = "00000000-0000-4000-8000-000000000001"
attempt = "00000000-0000-4000-8000-000000000002"


def frame(sequence, message):
    return ControlFrame.from_json({
        "protocol": {"major": 1, "minor": 0}, "request_id": request,
        "attempt_id": attempt, "sequence": str(sequence),
        "required_capabilities": [], "extensions": [], "message": message,
    })


with socket.socket(socket.AF_UNIX) as channel:
    channel.settimeout(10)
    channel.connect(sys.argv[1])
    with channel.makefile("rwb", buffering=0) as stream:
        hello = frame(0, {"type": "hello", "operation": "execute", "capabilities": []})
        print(json.dumps(hello.as_json()), flush=True)  # Deliberately looks like control JSON.
        print("synthetic diagnostic", file=sys.stderr, flush=True)
        write_frame(stream, hello)
        received = read_frame(stream)
        assert received is not None and received.as_json()["message"]["value"]["value"] == "18446744073709551615"
        write_frame(stream, frame(1, {"type": "completed"}))
