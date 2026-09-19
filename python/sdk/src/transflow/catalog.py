"""SDK-owned catalogue proxy. T010 prototype; no discovery or implicit binding."""

from ._catalog_prototype import CatalogRoot

C = CatalogRoot()

__all__ = ["C"]
