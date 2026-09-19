"""Index captured file names before any producer imports; helpers remain ordinary modules."""

import keyword
import sys
import unicodedata
from dataclasses import dataclass
from pathlib import PurePosixPath
from typing import Literal

_RUNTIME_MODULES = frozenset({"transflow", "transflow_worker", "polars", "duckdb", "pyarrow"})


class ModuleIndexError(ValueError):
    """A module collision or unsafe path, retaining paths separately from the safe heading."""

    def __init__(self, reason: str, paths: tuple[str, ...]) -> None:
        super().__init__(reason)
        self.paths = paths


@dataclass(frozen=True)
class ModuleEntry:
    """One importable producer/helper/package name; no code has been read or executed."""

    name: str
    path: str
    root: str
    kind: Literal["module", "package", "namespace"]


@dataclass(frozen=True)
class ModuleIndex:
    """Immutable index under the selected interpreter's identifier and stdlib rules."""

    modules: tuple[ModuleEntry, ...]
    resources: tuple[str, ...]
    python_minor: tuple[int, int]

    @classmethod
    def build(
        cls,
        source_roots: tuple[str, ...],
        captured_paths: tuple[str, ...],
        *,
        runtime_modules: frozenset[str] = frozenset(),
    ) -> "ModuleIndex":
        """Validate explicit capture paths without filesystem reads, imports or root-order tricks.

        The coordinator supplies its allowlisted capture paths. Additional required runtime
        modules are unioned with mandatory SDK/engine and interpreter stdlib names.
        Non-importable resource paths remain in the capture but are never imported.
        """
        if not 1 <= len(source_roots) <= 128 or len(captured_paths) > 100_000:
            raise ModuleIndexError("Source index exceeds its root or file limit", ())
        roots = tuple(_path(root) for root in source_roots)
        for i, root in enumerate(roots):
            if any(
                root.is_relative_to(other) or other.is_relative_to(root) for other in roots[i + 1 :]
            ):
                raise ModuleIndexError("Source roots overlap", tuple(source_roots))
        protected = _RUNTIME_MODULES | sys.stdlib_module_names | runtime_modules
        entries: dict[str, ModuleEntry] = {}
        resources: list[str] = []
        seen: set[str] = set()
        for path in sorted(captured_paths):
            relative = _path(path)
            if path in seen:
                raise ModuleIndexError("Capture repeats the same source path", (path,))
            seen.add(path)
            owners = [
                (text, root)
                for text, root in zip(source_roots, roots, strict=True)
                if relative.is_relative_to(root)
            ]
            if len(owners) != 1:
                raise ModuleIndexError(
                    "Capture path is outside the configured source roots", (path,)
                )
            root_text, root = owners[0]
            parts = relative.relative_to(root).parts
            if not parts or relative.suffix != ".py":
                resources.append(path)
                continue
            package = parts[-1] == "__init__.py"
            names = parts[:-1] if package else (*parts[:-1], relative.stem)
            if not names or any(
                not part.isidentifier() or keyword.iskeyword(part) for part in names
            ):
                resources.append(path)
                continue
            normalized = tuple(unicodedata.normalize("NFKC", part) for part in names)
            if normalized[0] in protected:
                raise ModuleIndexError(
                    "Source shadows the SDK, standard library or a required runtime dependency",
                    (path,),
                )
            if normalized != names:
                raise ModuleIndexError(
                    "Python normalizes this module name; use its normalized filename spelling",
                    (path,),
                )
            for i in range(1, len(names) + 1):
                name = ".".join(names[:i])
                terminal = i == len(names)
                entry = ModuleEntry(
                    name,
                    path if terminal else str(root.joinpath(*names[:i])),
                    root_text,
                    ("package" if package else "module") if terminal else "namespace",
                )
                previous = entries.get(name)
                if previous is None:
                    entries[name] = entry
                elif previous.root != entry.root:
                    raise ModuleIndexError(
                        "Two source roots contribute the same Python module or namespace",
                        (previous.path, path),
                    )
                elif previous.kind == "module" or entry.kind == "module":
                    raise ModuleIndexError(
                        "A Python module conflicts with another module or package",
                        (previous.path, path),
                    )
                elif previous.kind == "namespace" and entry.kind == "package":
                    entries[name] = entry
                elif previous.kind == "package" and entry.kind == "package":
                    raise ModuleIndexError(
                        "Two files declare the same Python package", (previous.path, path)
                    )
        return cls(
            tuple(entries[name] for name in sorted(entries)), tuple(resources), sys.version_info[:2]
        )


def _path(value: str) -> PurePosixPath:
    if (
        not isinstance(value, str)
        or not value
        or len(value.encode("utf-8")) > 4096
        or "\\" in value
        or any(ord(c) < 32 or ord(c) == 127 for c in value)
        or any(part in {"", ".", ".."} for part in value.split("/"))
    ):
        raise ModuleIndexError("Source paths must be normalized relative UTF-8 paths", ())
    path = PurePosixPath(value)
    if path.is_absolute():
        raise ModuleIndexError("Source paths must be workspace-relative", ())
    return path
