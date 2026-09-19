"""Private T010 fixture adapter. Runtime binding now uses production CatalogSnapshotV1."""

from __future__ import annotations

import keyword
import re
from uuid import UUID

from ._catalog import (
    CatalogContextError as CatalogContextError,
)
from ._catalog import (
    CatalogLookupError as CatalogLookupError,
)
from ._catalog import (
    CatalogNode as CatalogNode,
)
from ._catalog import (
    CatalogSnapshot,
    DatasetRef,
)
from ._catalog import (
    resolve_reference as resolve_reference,
)

PrototypeRef = DatasetRef


class PrototypeSnapshot(CatalogSnapshot):
    """Retain historical probe fixtures without a second runtime binding implementation."""

    __slots__ = ()

    def __init__(self, workspace_id: str, entries: tuple[tuple[str, str], ...]) -> None:
        from transflow_worker.canonical import catalog_fingerprint

        if str(UUID(workspace_id)) != workspace_id:
            raise ValueError("Workspace identity must be a canonical UUID")
        entries = tuple(sorted((path, dataset_id) for path, dataset_id in entries))
        if len(entries) > 10000:
            raise ValueError("Catalogue exceeds the prototype's 10000-entry limit")
        paths: set[str] = set()
        ids: set[str] = set()
        for path, dataset_id in entries:
            if not path or any(
                not re.fullmatch(r"[a-z][a-z0-9_]*", part) or keyword.iskeyword(part)
                for part in path.split("/")
            ):
                raise ValueError("Dataset paths must contain public, non-keyword identifiers")
            if str(UUID(dataset_id)) != dataset_id:
                raise ValueError("Dataset identity must be a canonical UUID")
            if path in paths or dataset_id in ids:
                raise ValueError("Duplicate dataset path or identity")
            paths.add(path)
            ids.add(dataset_id)
        document = {
            "format_version": 1,
            "workspace_id": workspace_id,
            "source_snapshot_id": str(UUID(int=99)),
            "catalog_fingerprint": "0" * 64,
            "entries": [
                {
                    "path": path,
                    "key": {"workspace_id": workspace_id, "dataset_id": dataset_id},
                    "kind": "transform",
                }
                for path, dataset_id in entries
            ],
        }
        document["catalog_fingerprint"] = catalog_fingerprint(document).hex
        super().__init__(document)

    @property
    def entries(self) -> tuple[tuple[str, str], ...]:
        """Historical local-only fixture shape; production uses paths and wire documents."""
        return tuple((path, self._references[path].dataset_id) for path in self.paths)
