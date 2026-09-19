"""Canonical JSON and purpose-separated SHA-256, profile v1.

Input is decoded JSON; duplicate keys must be rejected at the decode boundary.
No source selection, environment inspection, filesystem traversal or publication
occurs here. See schemas/canonical-v1.md for the byte-level contract.
"""

import hashlib
import json
import math
from dataclasses import dataclass
from enum import StrEnum
from typing import BinaryIO

from transflow_worker.wire import ProtocolError, validate_document

MAX_CANONICAL_BYTES = 16 * 1024 * 1024
MAX_SAFE_INTEGER = 9_007_199_254_740_991


class CanonicalError(ValueError):
    """Content violates the canonical profile or a selected fingerprint contract."""


class DigestKind(StrEnum):
    """The role is retained with each digest; FILE hashes raw bytes without a prefix."""

    ARTIFACT = "transflow.artifact.v1"
    SOURCE = "transflow.source.v1"
    COMPUTE = "transflow.compute.v1"
    CATALOG = "transflow.catalog.v1"
    SCHEMA = "transflow.schema.v1"
    FILE = ""


@dataclass(frozen=True)
class ContentDigest:
    """Purpose and lowercase hex content identity, never a publication UUID."""

    kind: DigestKind
    hex: str

    def __post_init__(self) -> None:
        if (
            not isinstance(self.kind, DigestKind)
            or len(self.hex) != 64
            or any(ch not in "0123456789abcdef" for ch in self.hex)
        ):
            raise CanonicalError("Invalid digest role or representation")


def canonical_json(value: object) -> bytes:
    """Serialize with bounded output/depth, exact strings and safe integer metadata."""
    output = bytearray()

    def append(data: bytes) -> None:
        if len(data) > MAX_CANONICAL_BYTES - len(output):
            raise CanonicalError("Canonical metadata exceeds its byte limit")
        output.extend(data)

    def string(text: str) -> None:
        if len(text) > MAX_CANONICAL_BYTES:
            raise CanonicalError("Canonical string exceeds its byte limit")
        append(b'"')
        # Bound temporary escaping allocations even for a large caller-supplied string.
        for offset in range(0, len(text), 256):
            try:
                append(json.dumps(text[offset : offset + 256], ensure_ascii=False)[1:-1].encode())
            except UnicodeEncodeError as error:
                raise CanonicalError("Canonical strings require Unicode scalars") from error
        append(b'"')

    def encode(item: object, depth: int) -> None:
        if depth > 64:
            raise CanonicalError("Canonical metadata exceeds its nesting limit")
        if item is None:
            append(b"null")
        elif isinstance(item, bool):
            append(b"true" if item else b"false")
        elif isinstance(item, int | float):
            if isinstance(item, float) and (not math.isfinite(item) or not item.is_integer()):
                raise CanonicalError("Bare numbers must be finite safe integers")
            if abs(item) > MAX_SAFE_INTEGER:
                raise CanonicalError("Use a lossless string carrier for large numbers")
            append(str(int(item)).encode("ascii"))
        elif isinstance(item, str):
            string(item)
        elif isinstance(item, list):
            append(b"[")
            for index, child in enumerate(item):
                if index:
                    append(b",")
                encode(child, depth + 1)
            append(b"]")
        elif isinstance(item, dict):
            if any(not isinstance(key, str) for key in item):
                raise CanonicalError("JSON object keys must be strings")
            append(b"{")
            for index, key in enumerate(sorted(item)):
                if index:
                    append(b",")
                string(key)
                append(b":")
                encode(item[key], depth + 1)
            append(b"}")
        else:
            raise CanonicalError("Only decoded JSON values can be canonicalized")

    encode(value, 0)
    return bytes(output)


def content_digest(kind: DigestKind, value: object) -> ContentDigest:
    """Hash prefix + NUL + canonical bytes; domain field selection is the caller's job."""
    if not isinstance(kind, DigestKind) or kind is DigestKind.FILE:
        raise CanonicalError("Use file_digest for unprefixed raw file bytes")
    digest = hashlib.sha256(kind.value.encode("ascii") + b"\0")
    digest.update(canonical_json(value))
    return ContentDigest(kind, digest.hexdigest())


def file_digest(reader: BinaryIO) -> ContentDigest:
    """Read every byte in 64 KiB chunks. I/O errors propagate, never yielding a partial hash."""
    digest = hashlib.sha256()
    while True:
        try:
            chunk = reader.read(64 * 1024)
        except InterruptedError:
            continue
        if not isinstance(chunk, bytes):
            raise CanonicalError("Raw content hashing requires a blocking byte stream")
        if not chunk:
            break
        if len(chunk) > 64 * 1024:
            raise CanonicalError("Reader exceeded the requested chunk size")
        digest.update(chunk)
    return ContentDigest(DigestKind.FILE, digest.hexdigest())


def _validate(name: str, value: object) -> None:
    try:
        validate_document(name, value)
    except ProtocolError as error:
        raise CanonicalError("Content does not match its fingerprint contract") from error


def schema_digest(value: object) -> ContentDigest:
    """Validate and fingerprint an ordered logical schema."""
    _validate("LogicalSchemaV1", value)
    return content_digest(DigestKind.SCHEMA, value)


def artifact_digest(value: object) -> ContentDigest:
    """Validate a manifest including its schema fingerprint; file bytes are checked by storage."""
    _validate("ArtifactManifestV1", value)
    if (
        not isinstance(value, dict)
        or value["schema_fingerprint"] != schema_digest(value["logical_schema"]).hex
    ):
        raise CanonicalError("Artifact schema fingerprint does not match")
    return content_digest(DigestKind.ARTIFACT, value)


def catalog_fingerprint(snapshot: object) -> ContentDigest:
    """Exclude the self-fingerprint/source snapshot ID; preserve declared entry order."""
    _validate("CatalogSnapshotV1", snapshot)
    if not isinstance(snapshot, dict):
        raise CanonicalError("A catalogue snapshot must be an object")
    return content_digest(
        DigestKind.CATALOG,
        {key: snapshot[key] for key in ("format_version", "workspace_id", "entries")},
    )


def verify_catalog_fingerprint(snapshot: object) -> None:
    """Reject an otherwise valid snapshot with a stale or tampered fingerprint."""
    digest = catalog_fingerprint(snapshot)
    if not isinstance(snapshot, dict) or snapshot["catalog_fingerprint"] != digest.hex:
        raise CanonicalError("Catalogue fingerprint does not match its content")


def semantic_fingerprint(kind: DigestKind, document: object) -> ContentDigest:
    """Hash only semantic in the closed {semantic, presentation} envelope.

    This explicit split never strips keys named description from user parameters.
    Full source/compute field selection is owned by later capture/planner services.
    """
    if kind not in (DigestKind.SOURCE, DigestKind.COMPUTE) or not isinstance(document, dict):
        raise CanonicalError("Expected a source or compute semantic envelope")
    if set(document) != {"semantic", "presentation"}:
        raise CanonicalError("Semantic envelopes contain exactly semantic and presentation")
    return content_digest(kind, document["semantic"])
