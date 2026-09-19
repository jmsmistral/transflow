"""Production codec failures and generated validator agreement; no engines needed."""

import io
import json
from pathlib import Path
from typing import Any

import pytest
from transflow import PROTOCOL_VERSION
from transflow_worker import _wire_validators as generated
from transflow_worker._version import SUPPORTED_PROTOCOL
from transflow_worker._wire_schema import PROTOCOL_MAJOR, PROTOCOL_MINOR
from transflow_worker.wire import (
    MAX_FRAME_BYTES,
    ControlFrame,
    ProtocolError,
    Session,
    read_frame,
    validate_document,
    write_frame,
)

CASES = json.loads(
    (Path(__file__).resolve().parents[2] / "schemas/fixtures/conformance.json").read_text()
)
REQUEST = "00000000-0000-4000-8000-000000000001"
ATTEMPT = "00000000-0000-4000-8000-000000000002"


def raw(sequence: int, message: dict[str, object]) -> dict[str, Any]:
    return {
        "protocol": {"major": 1, "minor": 0},
        "request_id": REQUEST,
        "attempt_id": ATTEMPT,
        "sequence": str(sequence),
        "required_capabilities": [],
        "extensions": [],
        "message": message,
    }


def hello() -> dict[str, object]:
    return {"type": "hello", "operation": "execute", "capabilities": []}


def frame(sequence: int, message: dict[str, object]) -> ControlFrame:
    return ControlFrame.from_json(raw(sequence, message))


def session() -> Session:
    return Session(REQUEST, ATTEMPT, "execute", frozenset())


@pytest.mark.parametrize("case", CASES, ids=[c["name"] for c in CASES])
def test_generated_runtime_validators_match_shared_corpus(case: dict[str, Any]) -> None:
    validator = getattr(generated, "validate_" + case["schema"])
    if case["valid"]:
        validator(case["value"])
    else:
        with pytest.raises(ProtocolError):
            validator(case["value"])


def test_unknown_contract_fails() -> None:
    assert (PROTOCOL_MAJOR, PROTOCOL_MINOR) == SUPPORTED_PROTOCOL
    assert (PROTOCOL_VERSION.major, PROTOCOL_VERSION.minor) == SUPPORTED_PROTOCOL
    with pytest.raises(ProtocolError, match="schema"):
        validate_document("unknown", {})


def test_frames_are_immutable_and_streams_round_trip() -> None:
    value = raw(0, hello())
    original = ControlFrame.from_json(value)
    value["message"]["operation"] = "discover"
    detached = original.as_json()
    detached["request_id"] = ATTEMPT
    assert original.as_json()["message"]["operation"] == "execute"
    assert original.request_id == REQUEST
    stream = io.BytesIO()
    for item in (original, frame(1, {"type": "completed"})):
        write_frame(stream, item)
    stream.seek(0)
    received = read_frame(stream)
    assert received is not None and received.as_json() == original.as_json()
    received = read_frame(stream)
    assert received is not None and received.message_type == "completed"
    assert read_frame(stream) is None


class Bytewise(io.BytesIO):
    def read(self, size: int | None = -1) -> bytes:
        return super().read(min(1, size if size is not None and size >= 0 else 1))

    def write(self, data: Any) -> int:
        return super().write(data[:1])


def test_partial_reads_and_writes_and_every_truncation() -> None:
    stream = Bytewise()
    write_frame(stream, frame(0, hello()))
    data = stream.getvalue()
    stream.seek(0)
    assert read_frame(stream) is not None
    for end in range(1, len(data)):
        with pytest.raises(ProtocolError, match="truncated"):
            read_frame(io.BytesIO(data[:end]))


@pytest.mark.parametrize("count", [0, MAX_FRAME_BYTES + 1, 2**32 - 1])
def test_oversized_prefix_is_rejected_before_body_read(count: int) -> None:
    with pytest.raises(ProtocolError, match="size"):
        read_frame(io.BytesIO(count.to_bytes(4, "big")))


def test_exact_byte_boundary_and_unicode_bytes() -> None:
    value = raw(0, {"type": "error", "code": "synthetic", "message": "", "retryable": False})
    baseline = len(json.dumps(value).encode())
    value["message"]["message"] = "x" * (MAX_FRAME_BYTES - baseline)
    exact = ControlFrame.from_json(value)
    assert len(exact.payload()) == MAX_FRAME_BYTES
    value["message"]["message"] += "x"
    with pytest.raises(ProtocolError, match="size"):
        ControlFrame.from_json(value)
    value["message"]["message"] = "🦀" * (MAX_FRAME_BYTES // 3)
    with pytest.raises(ProtocolError, match="size"):
        ControlFrame.from_json(value)


@pytest.mark.parametrize(
    "payload",
    [
        b"\xff",
        b"{} {}",
        b'{"x":1,"x":2}',
        b'{"x":{"a":1,"a":2}}',
        b'{"x":NaN}',
        b'{"x":1e999}',
        b'{"\\ud800":1}',
        b"[" * 2000 + b"0" + b"]" * 2000,
    ],
    ids=[
        "utf8",
        "trailing",
        "duplicate",
        "nested-duplicate",
        "nan",
        "overflow",
        "surrogate",
        "depth",
    ],
)
def test_strict_json_rejects_ambiguous_or_invalid_input(payload: bytes) -> None:
    with pytest.raises(ProtocolError, match="json"):
        ControlFrame(payload)


def test_unpaired_surrogates_and_unknown_required_semantics_fail() -> None:
    messages: list[dict[str, object]] = [
        {"type": "error", "code": "synthetic", "message": "\ud800", "retryable": False},
        {"type": "unknown"},
        {"type": "heartbeat", "required_new_semantics": True},
    ]
    for message in messages:
        with pytest.raises(ProtocolError):
            frame(0, message)
    value = raw(0, hello())
    value["protocol"]["major"] = 2
    with pytest.raises(ProtocolError, match="version"):
        ControlFrame.from_json(value)


def test_session_guards_identity_sequence_and_terminal_order() -> None:
    guard = session()
    with pytest.raises(ProtocolError, match="order"):
        guard.accept(frame(1, {"type": "heartbeat"}))
    assert guard.negotiated is None
    other = raw(0, hello())
    other["attempt_id"] = REQUEST
    with pytest.raises(ProtocolError, match="identity"):
        guard.accept(ControlFrame.from_json(other))
    guard.accept(frame(4, hello()))
    for sequence in (0, 4):
        with pytest.raises(ProtocolError, match="sequence"):
            guard.accept(frame(sequence, {"type": "heartbeat"}))
    with pytest.raises(ProtocolError, match="order"):
        guard.accept(frame(5, hello()))
    guard.accept(frame(10, {"type": "completed"}))
    with pytest.raises(ProtocolError, match="order"):
        guard.accept(frame(11, {"type": "heartbeat"}))


def test_capabilities_require_both_peers_even_at_newer_minor() -> None:
    guard = Session(REQUEST, ATTEMPT, "execute", frozenset(("diagnostic.note.v1",)))
    value = raw(0, hello())
    value["protocol"]["minor"] = 7
    value["message"]["capabilities"] = ["diagnostic.note.v1", "future.optional"]
    guard.accept(ControlFrame.from_json(value))
    assert guard.negotiated is not None
    assert guard.negotiated.minor == 0
    assert guard.negotiated.capabilities == {"diagnostic.note.v1"}
    extension = raw(1, {"type": "heartbeat"})
    extension["extensions"] = [
        {"capability": "diagnostic.note.v1", "value": {"type": "string", "value": "synthetic"}}
    ]
    guard.accept(ControlFrame.from_json(extension))
    missing = session()
    missing.accept(frame(0, hello()))
    with pytest.raises(ProtocolError, match="capability"):
        missing.accept(ControlFrame.from_json(extension))
    extension["required_capabilities"] = ["future.required"]
    with pytest.raises(ProtocolError, match="schema"):
        ControlFrame.from_json(extension)


def test_wrong_operation_and_duplicate_capabilities_do_not_advance_session() -> None:
    guard = session()
    for operation, capabilities in [("discover", []), ("execute", ["x", "x"])]:
        with pytest.raises(ProtocolError):
            guard.accept(
                frame(0, {"type": "hello", "operation": operation, "capabilities": capabilities})
            )
        assert guard.negotiated is None


def test_printed_json_is_not_a_control_frame() -> None:
    printed = frame(0, hello()).payload() + b"\n"
    with pytest.raises(ProtocolError, match="size"):
        read_frame(io.BytesIO(printed))
