"""Build handle arguments solely from the accepted, exact execution request."""

from __future__ import annotations

import importlib
import inspect
import json
from types import FunctionType
from typing import Any

from transflow.declarations import Declaration
from transflow.handles import InputDataset, OutputDataset


def arguments(
    function: Any,
    declaration: Declaration,
    request: dict[str, Any],
    frames: dict[str, Any],
    context: object,
) -> dict[str, Any]:
    from .polars_adapter import ExecutionError, _scan

    shape = FunctionType(
        function.__code__,
        function.__globals__,
        function.__name__,
        function.__defaults__,
        function.__closure__,
    )
    shape.__kwdefaults__ = function.__kwdefaults__
    signature = inspect.signature(shape, follow_wrapped=False, eval_str=False)
    values: dict[str, Any] = {}
    if declaration.handles:
        metadata = request["context"].get("datasets")
        if metadata is None:
            raise ExecutionError("bindings", "Enriched handles require accepted dataset metadata")
        if [m["alias"] for m in metadata["inputs"]] != [i["alias"] for i in request["inputs"]]:
            raise ExecutionError("bindings", "Handle metadata must match exact ordered inputs")
        for binding, meta in zip(request["inputs"], metadata["inputs"], strict=True):
            alias = binding["alias"]
            if alias not in signature.parameters:
                continue

            def dataframe(binding: dict[str, Any] = binding, alias: str = alias) -> Any:
                if alias not in frames:
                    pl = importlib.import_module("polars")

                    frames[alias] = _scan(pl, binding)
                return frames[alias]

            values[alias] = InputDataset(
                binding["dataset"]["dataset_id"],
                meta["path"],
                alias,
                metadata["branch"],
                meta["resolved_branch"],
                binding["version_id"],
                json.dumps(binding["manifest"]["logical_schema"]).encode(),
                dataframe,
            )
        if "output" in signature.parameters:
            destination = metadata["output"]
            values["output"] = OutputDataset(
                destination["dataset_id"],
                destination["path"],
                "output",
                metadata["branch"],
                None
                if declaration.output.schema is None
                else json.dumps(declaration.output.schema).encode(),
            )
    else:
        values = {
            alias: frames[alias] for alias, binding in declaration.inputs if binding.role == "data"
        }
    if "ctx" in signature.parameters:
        values["ctx"] = context
    return values
