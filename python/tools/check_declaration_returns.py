"""Real Polars return-boundary smoke in the already prepared qualification environment."""

import importlib
import json
import sys
from pathlib import Path

# Explicit repository SDK; never import authored workspaces or ambient cwd modules.
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "sdk" / "src"))
from transflow import DeclarationError, Output, transform  # noqa: E402
from transflow.declarations import validate_result  # noqa: E402


def main() -> None:
    polars = importlib.import_module("polars")

    @transform(output=Output("curated/items"))
    def function() -> object:
        return polars.DataFrame({"id": [1, 2]})

    frame = function()
    validate_result(function, frame)
    validate_result(function, polars.DataFrame({"id": [1, 2]}).lazy())
    rejected = 0
    for value in (None, [frame, frame], (x for x in range(2)), object(), 42):
        try:
            validate_result(function, value)
        except DeclarationError as exc:
            if "one DataFrame or LazyFrame" not in str(exc):
                raise
            rejected += 1
        else:
            raise RuntimeError("Unsupported return was accepted")
    print(
        json.dumps(
            {"result": "pass", "accepted": 2, "rejected": rejected, "polars": polars.__version__}
        )
    )


if __name__ == "__main__":
    main()
