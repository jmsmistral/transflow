"""Explicit test-only catalogue contexts; not a worker binding or discovery service."""

from collections.abc import Iterator
from contextlib import contextmanager

from ._catalog_prototype import CatalogContextError, PrototypeSnapshot, _binding


@contextmanager
def catalog_context(snapshot: PrototypeSnapshot, *, expected_fingerprint: str) -> Iterator[None]:
    """Bind a validated immutable snapshot for this test context and restore it on exit."""
    if snapshot.fingerprint != expected_fingerprint:
        raise CatalogContextError("Cannot bind a stale or mismatched catalogue fingerprint")
    token = _binding.set(snapshot)
    try:
        yield
    finally:
        _binding.reset(token)
