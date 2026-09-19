"""Explicit test contexts for direct Python imports; never changes a worker's lifetime binding."""

from collections.abc import Iterator
from contextlib import contextmanager

from ._catalog import (
    CatalogContextError,
    CatalogSnapshot,
    _require_testing_context,
    _test_binding,
)


@contextmanager
def catalog_context(snapshot: CatalogSnapshot, *, expected_fingerprint: str) -> Iterator[None]:
    """Bind a validated snapshot in this test context and restore it even after failure.

    Enter before importing your own catalogue-dependent modules. This does not clear
    Python's module cache or re-import a module already loaded by another test.
    """
    _require_testing_context()
    if not isinstance(snapshot, CatalogSnapshot) or snapshot.fingerprint != expected_fingerprint:
        raise CatalogContextError("Cannot bind a stale or mismatched catalogue fingerprint")
    token = _test_binding.set(snapshot)
    try:
        yield
    finally:
        _test_binding.reset(token)
