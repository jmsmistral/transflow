"""Bounded display-only expansion of a captured factory function; never execute it."""

from __future__ import annotations

import ast
import builtins
import copy
import inspect
from types import ModuleType
from typing import Any

from transflow._catalog import DatasetRef
from transflow.declarations import Declaration, Input, Output

MAX_SOURCE_BYTES = 512 * 1024
MAX_PREVIEW_BYTES = 128 * 1024


def _literal(value: object) -> ast.expr:
    # Discovery has already checked the immutable configuration contract.
    if type(value) is tuple:
        return ast.Tuple(elts=[_literal(item) for item in value], ctx=ast.Load())
    if value is None or isinstance(value, (bool, str, int, float)):
        return ast.Constant(value=value)
    raise ValueError("Unsupported preview literal")


class _Expand(ast.NodeTransformer):
    def __init__(self, values: dict[str, object]) -> None:
        self.values = values

    def visit_Name(self, node: ast.Name) -> ast.expr:
        if isinstance(node.ctx, ast.Load) and node.id in self.values:
            return ast.copy_location(_literal(self.values[node.id]), node)
        return node


def _binding(
    node: ast.expr,
    ref: str | DatasetRef,
    constructor: type[Input] | type[Output],
    globals_: dict[str, Any],
) -> bool:
    if not isinstance(node, ast.Call):
        return False
    target = globals_.get(node.func.id) if isinstance(node.func, ast.Name) else None
    if isinstance(node.func, ast.Attribute) and isinstance(node.func.value, ast.Name):
        owner = globals_.get(node.func.value.id)
        if isinstance(owner, ModuleType):
            target = vars(owner).get(node.func.attr)
    if target is not constructor:
        return False
    # A bound C reference retains its catalogue path in this explanatory view.
    value = ast.Constant(value=ref if isinstance(ref, str) else ref.path)
    if node.args:
        node.args[0] = value
        return True
    for keyword in node.keywords:
        if keyword.arg == "ref":
            keyword.value = value
            return True
    return False


def python_preview(function: object, declaration: Declaration, source: bytes) -> str | None:
    """Return an illustration only when this syntax can be expanded without guessing.

    Nested scopes, comprehensions and nonlocal/global writes need separate lexical
    analysis and are deliberately unavailable in this first preview. We preserve
    ordinary global names rather than serializing/importing their values.
    """
    if not inspect.isfunction(function) or len(source) > MAX_SOURCE_BYTES:
        return None
    try:
        module = ast.parse(source)
        if sum(1 for _ in ast.walk(module)) > 10000:
            return None
        candidates = [
            node
            for node in ast.walk(module)
            if isinstance(node, ast.FunctionDef)
            and node.name == function.__name__
            and min([node.lineno, *(d.lineno for d in node.decorator_list)])
            == function.__code__.co_firstlineno
        ]
        if len(candidates) != 1:
            return None
        parents = {
            child: parent for parent in ast.walk(module) for child in ast.iter_child_nodes(parent)
        }
        ancestor: ast.AST | None = parents.get(candidates[0])
        factory_locals: set[str] = set()
        while ancestor is not None:
            if isinstance(ancestor, (ast.FunctionDef, ast.AsyncFunctionDef)):
                factory_locals.update(
                    child.arg for child in ast.walk(ancestor.args) if isinstance(child, ast.arg)
                )
                factory_locals.update(
                    child.id
                    for child in ast.walk(ancestor)
                    if isinstance(child, ast.Name) and isinstance(child.ctx, ast.Store)
                )
            ancestor = parents.get(ancestor)
        node = copy.deepcopy(candidates[0])
        scoped = (
            ast.FunctionDef,
            ast.AsyncFunctionDef,
            ast.ClassDef,
            ast.Lambda,
            ast.ListComp,
            ast.SetComp,
            ast.DictComp,
            ast.GeneratorExp,
            ast.Nonlocal,
            ast.Global,
        )
        if any(isinstance(child, scoped) for child in ast.walk(node) if child is not node):
            return None
        if len(node.decorator_list) != 1 or not isinstance(node.decorator_list[0], ast.Call):
            return None
        decorator = node.decorator_list[0]
        keywords = {keyword.arg: keyword.value for keyword in decorator.keywords}
        bindings: list[tuple[str, Input | Output]] = [
            *declaration.inputs,
            ("output", declaration.output),
        ]
        for alias, binding in bindings:
            if alias not in keywords or not _binding(
                keywords[alias], binding.ref, type(binding), function.__globals__
            ):
                return None
        values = {
            name: cell.cell_contents
            for name, cell in zip(
                function.__code__.co_freevars, function.__closure__ or (), strict=True
            )
        }
        # Definition-time defaults may differ from the factory's original expression.
        node.args.defaults = [_literal(value) for value in function.__defaults__ or ()]
        defaults = function.__kwdefaults__ or {}
        node.args.kw_defaults = [
            _literal(defaults[arg.arg]) if arg.arg in defaults else None
            for arg in node.args.kwonlyargs
        ]
        expanded = _Expand(values).visit(node)
        if not isinstance(expanded, ast.FunctionDef):
            return None
        node = expanded
        available = set(function.__globals__) | set(vars(builtins))
        if any(
            isinstance(child, ast.Name)
            and isinstance(child.ctx, ast.Load)
            and (child.id not in available or child.id in factory_locals)
            for child in ast.walk(node.decorator_list[0])
        ):
            # For example, a decorator-only local check expression not in the closure.
            return None
        text = (
            "# Expanded Python preview, reconstructed from captured factory values.\n"
            "# Not executed source. Imports and global helpers remain in Authored.\n"
            + ast.unparse(ast.fix_missing_locations(node))
            + "\n"
        )
        if len(text.encode()) > MAX_PREVIEW_BYTES or len(text.splitlines()) > 2000:
            return None
        return text
    except SyntaxError, ValueError, RecursionError:
        return None


def execution_declaration(value: dict[str, Any]) -> dict[str, Any]:
    """Display metadata must not change accepted execution or legacy replay equality."""
    result = dict(value)
    if isinstance(value.get("authoring"), dict):
        result["authoring"] = dict(value["authoring"])
        result["authoring"].pop("python_preview", None)
    return result
