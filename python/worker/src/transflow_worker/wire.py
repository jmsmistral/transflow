"""Bounded private-channel framing and negotiation; no worker operations are dispatched."""

import json
import math
from dataclasses import dataclass
from typing import Any, BinaryIO, Self

from ._wire_assertions import validate
from ._wire_schema import PROTOCOL_MAJOR, PROTOCOL_MINOR, SCHEMA_TEXT

MAX_FRAME_BYTES = 1024 * 1024
SCHEMA = json.loads(SCHEMA_TEXT)  # Trusted generated schema data, never peer-provided.
OPERATIONS = frozenset(
    (
        "discover",
        "execute",
        "evaluate_checks",
        "query_preview",
        "infer_lineage",
        "inspect_environment",
    )
)


class ProtocolError(ValueError):
    """Safe protocol classification; arbitrary payload text is never included."""

    def __init__(self, code: str) -> None:
        self.code = code
        super().__init__(f"Worker control protocol rejected input ({code})")


def validate_document(name: str, value: object) -> None:
    """Validate a named generated contract, including asserted semantic formats."""
    definition = SCHEMA["$defs"].get(name)
    if definition is None:
        raise ProtocolError("schema")
    try:
        validate(definition, value, SCHEMA["$defs"])
    except (ValueError, TypeError, OverflowError, RecursionError) as exc:
        raise ProtocolError("schema") from exc


def _pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        key.encode("utf-8", errors="strict")
        if key in result:
            raise ProtocolError("json")
        result[key] = value
    return result


def _constant(_: str) -> None:
    raise ProtocolError("json")


def _float(text: str) -> float:
    value = float(text)
    if not math.isfinite(value):
        raise ProtocolError("json")
    return value


def _json_limits(value: object, depth: int = 0) -> None:
    if depth > 64:
        raise ProtocolError("json")
    if isinstance(value, str):
        value.encode("utf-8", errors="strict")
    elif isinstance(value, list):
        for item in value:
            _json_limits(item, depth + 1)
    elif isinstance(value, dict):
        for item in value.values():
            _json_limits(item, depth + 1)


def _decode(payload: bytes) -> dict[str, Any]:
    if not isinstance(payload, bytes) or not 0 < len(payload) <= MAX_FRAME_BYTES:
        raise ProtocolError("size")
    try:
        result = json.loads(
            payload.decode("utf-8"),
            object_pairs_hook=_pairs,
            parse_constant=_constant,
            parse_float=_float,
        )
        _json_limits(result)
        if not isinstance(result, dict):
            raise ProtocolError("schema")
        return result
    except (ValueError, UnicodeError, RecursionError) as exc:
        if isinstance(exc, ProtocolError):
            raise
        raise ProtocolError("json") from exc


@dataclass(frozen=True, slots=True, init=False)
class ControlFrame:
    """Immutable payload and validated facts; every outward mapping is a fresh copy."""

    _payload: bytes
    request_id: str
    attempt_id: str
    sequence: int
    minor: int
    message_type: str

    def __init__(self, payload: bytes) -> None:
        value = _decode(payload)
        protocol = value.get("protocol")
        if isinstance(protocol, dict):
            major = protocol.get("major")
            if type(major) in (int, float) and major != PROTOCOL_MAJOR:
                raise ProtocolError("version")
        validate_document("ControlFrameV1", value)
        object.__setattr__(self, "_payload", payload)
        object.__setattr__(self, "request_id", value["request_id"])
        object.__setattr__(self, "attempt_id", value["attempt_id"])
        object.__setattr__(self, "sequence", int(value["sequence"]))
        object.__setattr__(self, "minor", int(value["protocol"]["minor"]))
        object.__setattr__(self, "message_type", value["message"]["type"])

    @classmethod
    def from_json(cls, value: object) -> Self:
        """Encode with a byte cap, then apply strict decoding and the shared contract."""
        buffer = bytearray()
        try:
            for chunk in json.JSONEncoder(ensure_ascii=False, allow_nan=False).iterencode(value):
                # Reject an oversized string before creating another large encoded byte copy.
                if len(chunk) > MAX_FRAME_BYTES - len(buffer):
                    raise ProtocolError("size")
                encoded = chunk.encode("utf-8")
                if len(encoded) > MAX_FRAME_BYTES - len(buffer):
                    raise ProtocolError("size")
                buffer.extend(encoded)
        except (ValueError, TypeError, UnicodeError, RecursionError) as exc:
            if isinstance(exc, ProtocolError):
                raise
            raise ProtocolError("json") from exc
        return cls(bytes(buffer))

    def as_json(self) -> dict[str, Any]:
        """Return detached data; caller mutations cannot change this validated frame."""
        return _decode(self._payload)

    def payload(self) -> bytes:
        """Return immutable, validated JSON bytes (not canonical hashing bytes)."""
        return self._payload


def _read_exact(reader: BinaryIO, count: int) -> bytes:
    buffer = bytearray()
    while len(buffer) < count:
        part = reader.read(count - len(buffer))
        if not part:
            raise ProtocolError("truncated")
        buffer.extend(part)
    return bytes(buffer)


def read_frame(reader: BinaryIO) -> ControlFrame | None:
    """Read a private stream; clean EOF differs from truncated prefix/payload."""
    try:
        first = reader.read(1)
        if not first:
            return None
        count = int.from_bytes(first + _read_exact(reader, 3), "big")
        if not 0 < count <= MAX_FRAME_BYTES:
            raise ProtocolError("size")
        return ControlFrame(_read_exact(reader, count))
    except OSError as exc:
        raise ProtocolError("io") from exc


def write_frame(writer: BinaryIO, frame: ControlFrame) -> None:
    """Write only to the supplied channel; stdout/stderr are never consulted."""
    data = len(frame.payload()).to_bytes(4, "big") + frame.payload()
    offset = 0
    try:
        while offset < len(data):
            written = writer.write(data[offset:])
            if written is None or written <= 0:
                raise ProtocolError("io")
            offset += written
    except OSError as exc:
        raise ProtocolError("io") from exc


@dataclass(frozen=True, slots=True)
class Negotiated:
    """Explicitly agreed minor and capabilities, not implicit execution support."""

    minor: int
    capabilities: frozenset[str]


class Session:
    """Receive guard for one request/attempt. Nonce authentication belongs to T055."""

    def __init__(
        self, request_id: str, attempt_id: str, operation: str, capabilities: frozenset[str]
    ) -> None:
        validate_document("Uuid", request_id)
        validate_document("Uuid", attempt_id)
        if operation not in OPERATIONS:
            raise ProtocolError("order")
        if not capabilities <= {"diagnostic.note.v1", "discovery.v1", "polars.execute.v1"}:
            raise ProtocolError("capability")
        self._request = request_id
        self._attempt = attempt_id
        self._operation = operation
        self._local = frozenset(capabilities)
        self._negotiated: Negotiated | None = None
        self._last: int | None = None
        self._terminal = False

    @property
    def negotiated(self) -> Negotiated | None:
        """None until a complete valid hello was accepted."""
        return self._negotiated

    def accept(self, frame: ControlFrame) -> None:
        """Reject mismatched IDs, replay, missing hello or unnegotiated semantics."""
        if (frame.request_id, frame.attempt_id) != (self._request, self._attempt):
            raise ProtocolError("identity")
        if self._last is not None and frame.sequence <= self._last:
            raise ProtocolError("sequence")
        if self._terminal:
            raise ProtocolError("order")
        value = frame.as_json()
        negotiated = self._negotiated
        if frame.message_type == "hello":
            if negotiated is not None or value["message"]["operation"] != self._operation:
                raise ProtocolError("order")
            peer = value["message"]["capabilities"]
            if len(set(peer)) != len(peer):
                raise ProtocolError("capability")
            negotiated = Negotiated(min(PROTOCOL_MINOR, frame.minor), self._local & set(peer))
        if negotiated is None:
            raise ProtocolError("order")
        required = set(value["required_capabilities"]) | {
            e["capability"] for e in value["extensions"]
        }
        if frame.message_type == "discovery_ready":
            if self._operation != "discover":
                raise ProtocolError("order")
            required.add("discovery.v1")
        if not required <= negotiated.capabilities:
            raise ProtocolError("capability")
        self._negotiated = negotiated
        self._last = frame.sequence
        self._terminal = frame.message_type in ("completed", "error")
