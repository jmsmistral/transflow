"""No producer imports are needed to validate the complete import namespace."""

import sys
from dataclasses import FrozenInstanceError
from pathlib import Path

import pytest
from transflow_worker.source_index import ModuleIndex, ModuleIndexError


def test_helpers_packages_resources_and_unicode_are_indexed_without_execution() -> None:
    paths = (
        "src/common/__init__.py",
        "src/common/identifiers.py",
        "src/transforms/orders.py",
        "sql/query.sql",
        "src/λ.py",
        "src/non-importable/helper.py",
    )
    index = ModuleIndex.build(("src", "sql"), paths)
    assert [m.name for m in index.modules] == [
        "common",
        "common.identifiers",
        "transforms",
        "transforms.orders",
        "λ",
    ]
    assert index.modules[0].kind == "package"
    assert index.resources == ("sql/query.sql", "src/non-importable/helper.py")
    assert index.python_minor == sys.version_info[:2]
    assert ModuleIndex.build(("src", "sql"), tuple(reversed(paths))) == index
    with pytest.raises(FrozenInstanceError):
        setattr(index.modules[0], "".join(("na", "me")), "changed")


@pytest.mark.parametrize(
    "paths",
    [
        ("src/common/one.py", "other/common/two.py"),
        ("src/common.py", "src/common/__init__.py"),
        ("src/common.py", "src/common/child.py"),
        ("src/common.py", "other/common.py"),
    ],
)
def test_collisions_report_both_paths(paths: tuple[str, ...]) -> None:
    with pytest.raises(ModuleIndexError) as error:
        ModuleIndex.build(("src", "other"), paths)
    assert len(error.value.paths) == 2


@pytest.mark.parametrize(
    "name", ["transflow", "transflow_worker", "polars", "json", "os", "asyncio", "ｏｓ"]
)
def test_shadowing_fails_before_import(name: str) -> None:
    with pytest.raises(ModuleIndexError, match="shadows"):
        ModuleIndex.build(("src",), (f"src/{name}.py",))


def test_runtime_dependency_profile_can_add_but_not_remove_protected_names() -> None:
    with pytest.raises(ModuleIndexError, match="shadows"):
        ModuleIndex.build(
            ("src",),
            ("src/custom_dependency/__init__.py",),
            runtime_modules=frozenset({"custom_dependency"}),
        )
    with pytest.raises(ModuleIndexError, match="shadows"):
        ModuleIndex.build(("src",), ("src/transflow.py",), runtime_modules=frozenset())


@pytest.mark.parametrize(
    "path",
    [
        "../escape.py",
        "/absolute.py",
        "src/../escape.py",
        "src//file.py",
        "src/file\x00.py",
        "outside/file.py",
    ],
)
def test_unsafe_paths_are_not_importable(path: str) -> None:
    with pytest.raises(ModuleIndexError):
        ModuleIndex.build(("src",), (path,))


def test_empty_helper_only_index_is_valid_and_duplicate_roots_are_not() -> None:
    assert ModuleIndex.build(("src",), ()).modules == ()
    assert len(ModuleIndex.build(("src",), ("src/helper.py",)).modules) == 1
    for roots in [("src", "src/nested"), ("src", "src")]:
        with pytest.raises(ModuleIndexError, match="overlap"):
            ModuleIndex.build(roots, ())
    with pytest.raises(ModuleIndexError, match="repeats"):
        ModuleIndex.build(("src",), ("src/helper.py", "src/helper.py"))


def test_nested_helpers_use_normal_imports_from_the_explicit_root(tmp_path: Path) -> None:
    import subprocess

    root = tmp_path / "src"
    (root / "common").mkdir(parents=True)
    (root / "common" / "__init__.py").write_text("")
    (root / "common" / "identifiers.py").write_text("def normalize(value): return value.strip()\n")
    (root / "producer.py").write_text(
        "from common.identifiers import normalize\nresult=normalize(' ok ')\n"
    )
    index = ModuleIndex.build(
        ("src",), ("src/common/__init__.py", "src/common/identifiers.py", "src/producer.py")
    )
    assert len(index.modules) == 3
    result = subprocess.run(
        [
            sys.executable,
            "-I",
            "-c",
            "import sys; sys.path.insert(0, sys.argv[1]); "
            "import producer; assert producer.result == 'ok'",
            str(root),
        ],
        capture_output=True,
        timeout=10,
        check=False,
    )
    assert result.returncode == 0, result.stderr
