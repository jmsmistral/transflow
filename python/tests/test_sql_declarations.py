"""SQL registrations stay pure and captured discovery owns file resolution."""

import dataclasses
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest
from transflow import DeclarationError, Input, Output, sql_transform
from transflow.declarations import get_declaration


def test_import_and_registration_do_not_load_engine_or_read_sql() -> None:
    sdk = str(Path(__file__).resolve().parents[1] / "sdk/src")
    code = (
        f"import sys\nsys.path.insert(0, {sdk!r})\n"
        + """import sys
from transflow import sql_transform, Input, Output
result = sql_transform('absent.sql', rows=Input('raw/items'), output=Output('curated/items'))
assert not callable(result)
assert not {'duckdb', 'polars', 'pyarrow'} & sys.modules.keys()
"""
    )
    result = subprocess.run(
        [sys.executable, "-c", code], capture_output=True, text=True, check=False
    )
    assert result.returncode == 0, result.stderr


@pytest.mark.parametrize(
    "path", ["../outside.sql", "/outside.sql", "x.py", "x\\bad.sql", "./x.sql"]
)
def test_sql_path_is_a_captured_relative_resource(path: str) -> None:
    with pytest.raises(DeclarationError, match="relative"):
        sql_transform(path, output=Output("curated/items"))


def test_sql_registration_requires_module_scope() -> None:
    with pytest.raises(DeclarationError, match="module scope"):
        sql_transform("items.sql", rows=Input("raw/items"), output=Output("curated/items"))


def test_sql_registration_is_immutable() -> None:
    namespace: dict[str, Any] = {"__name__": "test_sql_producer"}
    exec(
        "from transflow import sql_transform, Output\n"
        "result=sql_transform('items.sql',output=Output('curated/items'))",
        namespace,
    )
    result = namespace["result"]
    declaration = get_declaration(result)
    assert declaration is not None and declaration.engine == "duckdb"
    with pytest.raises(dataclasses.FrozenInstanceError):
        result.declaration = None
