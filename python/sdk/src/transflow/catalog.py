"""SDK-owned immutable catalogue references; no discovery or implicit workspace selection."""

from ._catalog import (
    CatalogContextError,
    CatalogLookupError,
    CatalogRoot,
    CatalogSnapshot,
    DatasetRef,
    resolve_reference,
)

C = CatalogRoot()

__all__ = [
    "C",
    "CatalogSnapshot",
    "DatasetRef",
    "CatalogContextError",
    "CatalogLookupError",
    "resolve_reference",
]
