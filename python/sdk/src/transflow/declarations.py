"""Pure immutable authoring declarations; decorators leave Python callables unchanged."""

from __future__ import annotations

import importlib
import inspect
import json
import keyword
import re
import unicodedata
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field, replace
from datetime import datetime
from enum import Enum
from types import FunctionType
from typing import Protocol, TypeVar
from uuid import UUID

from ._catalog import CatalogNode, DatasetRef, capture_reference
from ._declaration_values import DeclarationError, Parameter, frozen_json
from .expectations import Expectation


class Branch(Enum):
    CURRENT = "current"


def _name(value: object, label: str) -> str:
    if (
        not isinstance(value, str)
        or not 0 < len(value) <= 256
        or any(ord(c) < 32 or ord(c) == 127 for c in value)
    ):
        raise DeclarationError(f"{label} must be nonempty text without control characters")
    return value


def _alias(value: str) -> None:
    if (
        not isinstance(value, str)
        or not value.isidentifier()
        or keyword.iskeyword(value)
        or value.startswith("_")
        or unicodedata.normalize("NFKC", value) != value
    ):
        raise DeclarationError("Input/parameter alias must be a public Python identifier")


def _reference(value: str | CatalogNode) -> str | DatasetRef:
    if isinstance(value, CatalogNode):
        return capture_reference(value)
    if not isinstance(value, str):
        raise DeclarationError(
            "Dataset reference must be a path, dataset:UUID or bound C reference"
        )
    if value.startswith("dataset:"):
        try:
            if str(UUID(value[8:])) == value[8:]:
                return value
        except ValueError:
            pass
        raise DeclarationError("Dataset identity must be a canonical dataset:UUID")
    if not value or any(
        re.fullmatch(r"[a-z][a-z0-9_]*", part) is None or keyword.iskeyword(part)
        for part in value.split("/")
    ):
        raise DeclarationError("Dataset path requires slash-separated lowercase identifiers")
    return value


def _nonnegative(value: object, label: str) -> None:
    if type(value) is not int or value < 0:
        raise DeclarationError(f"{label} must be a nonnegative integer")


@dataclass(frozen=True, slots=True)
class Check:
    expectation: Expectation
    name: str
    id: str | None = field(default=None, kw_only=True)
    on_error: str = field(default="FAIL", kw_only=True)
    null_policy: str | None = field(default=None, kw_only=True)
    sample_rows: int | None = field(default=None, kw_only=True)
    description: str | None = field(default=None, kw_only=True)

    def __post_init__(self) -> None:
        if not isinstance(self.expectation, Expectation):
            raise DeclarationError(
                "Check requires a typed expectation, not SQL, a lambda or engine expression"
            )
        _name(self.name, "Check name")
        if self.on_error not in ("FAIL", "WARN"):
            raise DeclarationError("Check on_error must be exactly FAIL or WARN")
        if self.null_policy not in (None, "fail", "ignore"):
            raise DeclarationError("Check null_policy must be fail, ignore or None for inheritance")
        if self.sample_rows is not None:
            _nonnegative(self.sample_rows, "Check sample_rows")
        if self.description is not None:
            _name(self.description, "Check description")
        identity = self.id
        if identity is None:
            identity = re.sub(r"[^\w]+", "_", self.name.casefold(), flags=re.UNICODE).strip("_")
            if not identity:
                raise DeclarationError("This check name needs an explicit nonempty id")
        _name(identity, "Check id")
        object.__setattr__(self, "id", identity)

    def effective(self, *, null_policy: str = "fail", sample_rows: int = 0) -> Check:
        """Resolve inheritance before a later planner fingerprints check semantics."""
        if null_policy not in ("fail", "ignore"):
            raise DeclarationError("Workspace null_policy must be fail or ignore")
        _nonnegative(sample_rows, "Workspace sample_rows")
        return replace(
            self,
            null_policy=self.null_policy if self.null_policy is not None else null_policy,
            sample_rows=self.sample_rows if self.sample_rows is not None else sample_rows,
        )


def _checks(value: object) -> tuple[Check, ...]:
    result: tuple[Check, ...]
    if isinstance(value, Check):
        result = (value,)
    elif isinstance(value, Sequence) and not isinstance(value, (str, bytes)):
        result = tuple(value)
    else:
        raise DeclarationError("checks must be a Check or a sequence of Check declarations")
    if any(not isinstance(check, Check) for check in result):
        raise DeclarationError("checks must contain only Check declarations")
    if len({check.id for check in result}) != len(result):
        raise DeclarationError("Check IDs must be unique within each input or output binding")
    return result


@dataclass(frozen=True, slots=True, init=False)
class Input:
    ref: str | DatasetRef
    branch: str | Branch | None
    stop_branch_fallback: bool
    checks: tuple[Check, ...]
    role: str

    def __init__(
        self,
        ref: str | CatalogNode,
        *,
        branch: str | Branch | None = None,
        stop_branch_fallback: bool = False,
        checks: Check | Sequence[Check] = (),
        role: str = "data",
    ) -> None:
        if branch is not None and branch is not Branch.CURRENT:
            if (
                not isinstance(branch, str)
                or not branch
                or any(unicodedata.category(c) == "Cc" for c in branch)
            ):
                raise DeclarationError("Input branch must be nonempty text without controls")
        if type(stop_branch_fallback) is not bool:
            raise DeclarationError("stop_branch_fallback must be a boolean")
        if role not in ("data", "validation"):
            raise DeclarationError("Input role must be data or validation")
        object.__setattr__(self, "ref", _reference(ref))
        object.__setattr__(self, "branch", branch)
        object.__setattr__(self, "stop_branch_fallback", stop_branch_fallback)
        object.__setattr__(self, "checks", _checks(checks))
        object.__setattr__(self, "role", role)


@dataclass(frozen=True, slots=True, init=False)
class Output:
    ref: str | DatasetRef
    checks: tuple[Check, ...]
    _schema: bytes | None

    def __init__(
        self, ref: str | CatalogNode, *, checks: Check | Sequence[Check] = (), schema: object = None
    ) -> None:
        reference = _reference(ref)
        path = reference if isinstance(reference, str) else reference.path
        if path.split("/")[0] == "external":
            raise DeclarationError("Outputs must be local; external datasets are read boundaries")
        object.__setattr__(self, "ref", reference)
        object.__setattr__(self, "checks", _checks(checks))
        object.__setattr__(
            self, "_schema", None if schema is None else frozen_json(schema, "LogicalSchemaV1")
        )

    @property
    def schema(self) -> object:
        return None if self._schema is None else json.loads(self._schema)


class TransformContext(Protocol):
    """Execution-owned context interface; no implicit runtime is created by decorators."""

    @property
    def params(self) -> Mapping[str, object]: ...

    @property
    def build_id(self) -> str: ...

    @property
    def job_id(self) -> str: ...

    @property
    def attempt_id(self) -> str: ...

    @property
    def cancelled(self) -> bool: ...

    @property
    def evaluation_time(self) -> datetime: ...

    @property
    def random_seed(self) -> int | None: ...

    def log(self, message: str) -> None: ...

    def secret(self, reference: str) -> str: ...


@dataclass(frozen=True, slots=True)
class Declaration:
    output: Output
    inputs: tuple[tuple[str, Input], ...]
    engine: str
    params: tuple[tuple[str, Parameter], ...]
    resources: tuple[tuple[str, int], ...]
    cache: str | None
    lineage_json: bytes | None
    refresh: str | int | None
    secret_refs: tuple[str, ...]
    source: bool
    module: str
    qualname: str
    filename: str
    line: int


F = TypeVar("F", bound=Callable[..., object])
_ATTRIBUTE = "__transflow_declaration__"
_RESERVED = {"output", "engine", "params", "resources", "cache", "lineage", "ctx"}


def get_declaration(function: object) -> Declaration | None:
    """Helpers have no declaration; discovery decides module-level producer rules."""
    result = getattr(function, _ATTRIBUTE, None)
    if result is not None and not isinstance(result, Declaration):
        raise DeclarationError("Callable has invalid Transflow declaration metadata")
    return result


def _decorate(
    *,
    output: Output,
    inputs: Mapping[str, Input],
    engine: str,
    params: Mapping[str, Parameter] | None,
    resources: Mapping[str, int] | None,
    cache: str | None,
    lineage: object,
    refresh: str | int | None,
    secret_refs: Sequence[str],
    source: bool,
) -> Callable[[F], F]:
    if not isinstance(output, Output):
        raise DeclarationError("Declare exactly one output=Output(...)")
    if engine != "polars":
        raise DeclarationError("Only the polars transform engine is currently supported")
    if cache not in (None, "deterministic", "never"):
        raise DeclarationError("cache must be deterministic or never")
    bound = tuple(inputs.items())
    for alias, value in bound:
        _alias(alias)
        if alias in _RESERVED or not isinstance(value, Input):
            raise DeclarationError(
                f"Input alias {alias!r} is reserved or is not an Input declaration"
            )
    if params is not None and not isinstance(params, Mapping):
        raise DeclarationError("params must be a mapping of typed Parameter declarations")
    parameters = tuple((params or {}).items())
    for name, parameter in parameters:
        _alias(name)
        if not isinstance(parameter, Parameter):
            raise DeclarationError("params values must be typed Parameter declarations")
    if resources is not None and not isinstance(resources, Mapping):
        raise DeclarationError("resources must be a mapping of resource overrides")
    limits = tuple((resources or {}).items())
    for name, limit in limits:
        if name not in {"wall_timeout_seconds"}:
            raise DeclarationError("Unsupported resource override; use wall_timeout_seconds")
        _nonnegative(limit, name)
    if isinstance(secret_refs, (str, bytes)):
        raise DeclarationError("secret_refs must be a sequence of declared reference names")
    secrets = tuple(secret_refs)
    for name in secrets:
        _name(name, "Secret reference")
    if len(set(secrets)) != len(secrets):
        raise DeclarationError("secret_refs must not repeat reference names")
    lineage_json = None if lineage is None else frozen_json(lineage)
    aliases = {name for name, _ in bound}
    bindings: tuple[Input | Output, ...] = (output, *(value for _, value in bound))
    for binding in bindings:
        for check in binding.checks:
            check.expectation.validate_inputs(aliases)

    def decorate(function: F) -> F:
        if (
            not inspect.isfunction(function)
            or inspect.iscoroutinefunction(function)
            or inspect.isgeneratorfunction(function)
        ):
            raise DeclarationError(
                "Decorate an ordinary synchronous Python function returning one Polars table"
            )
        if get_declaration(function) is not None:
            raise DeclarationError("A function cannot have multiple producer declarations")
        # Inspect only call mechanics. Even Python 3.14's STRING annotation
        # format can execute authored code. A temporary function with the same
        # code/defaults and no annotation hook avoids evaluating any annotations.
        shape = FunctionType(
            function.__code__,
            function.__globals__,
            function.__name__,
            function.__defaults__,
            function.__closure__,
        )
        shape.__kwdefaults__ = function.__kwdefaults__
        signature = inspect.signature(shape, follow_wrapped=False, eval_str=False)
        data = {name for name, value in bound if value.role == "data"}
        for name, parameter in signature.parameters.items():
            if parameter.kind in (
                parameter.VAR_POSITIONAL,
                parameter.VAR_KEYWORD,
                parameter.POSITIONAL_ONLY,
            ):
                raise DeclarationError(
                    "Transform signatures cannot use positional-only arguments, *args or **kwargs"
                )
            if name == "ctx":
                if parameter.kind != parameter.KEYWORD_ONLY:
                    raise DeclarationError("ctx must be keyword-only; annotate it TransformContext")
            elif name not in data:
                raise DeclarationError(
                    f"Function parameter {name!r} has no role=data Input declaration; "
                    "use ctx.params"
                )
        missing = data - signature.parameters.keys()
        if missing:
            raise DeclarationError(
                "Function is missing data Input aliases: " + ", ".join(sorted(missing))
            )
        declaration = Declaration(
            output,
            bound,
            engine,
            parameters,
            limits,
            cache,
            lineage_json,
            refresh,
            secrets,
            source,
            function.__module__,
            function.__qualname__,
            function.__code__.co_filename,
            function.__code__.co_firstlineno,
        )
        setattr(function, _ATTRIBUTE, declaration)
        return function

    return decorate


def transform(
    *,
    output: Output,
    engine: str = "polars",
    params: Mapping[str, Parameter] | None = None,
    resources: Mapping[str, int] | None = None,
    cache: str = "deterministic",
    lineage: object = None,
    **named_inputs: Input,
) -> Callable[[F], F]:
    return _decorate(
        output=output,
        inputs=named_inputs,
        engine=engine,
        params=params,
        resources=resources,
        cache=cache,
        lineage=lineage,
        refresh=None,
        secret_refs=(),
        source=False,
    )


def source_transform(
    *,
    output: Output,
    engine: str = "polars",
    params: Mapping[str, Parameter] | None = None,
    resources: Mapping[str, int] | None = None,
    refresh: str | Mapping[str, int] = "always",
    secret_refs: Sequence[str] = (),
) -> Callable[[F], F]:
    policy: str | int
    if isinstance(refresh, Mapping):
        if set(refresh) != {"ttl_seconds"}:
            raise DeclarationError("Source refresh mapping requires only ttl_seconds")
        ttl = refresh["ttl_seconds"]
        _nonnegative(ttl, "Source ttl_seconds")
        if ttl == 0:
            raise DeclarationError("Source ttl_seconds must be positive")
        policy = ttl
    elif refresh in ("always", "manual"):
        policy = refresh
    else:
        raise DeclarationError("Source refresh must be always, manual or positive ttl_seconds")
    return _decorate(
        output=output,
        inputs={},
        engine=engine,
        params=params,
        resources=resources,
        cache=None,
        lineage=None,
        refresh=policy,
        secret_refs=secret_refs,
        source=True,
    )


def validate_result(function: object, result: object) -> None:
    """Explicit execution boundary; direct decorated calls never invoke this check."""
    if get_declaration(function) is None:
        raise DeclarationError("Return validation requires a decorated producer")
    try:
        pl = importlib.import_module("polars")
    except ImportError as exc:
        raise DeclarationError(
            "Polars is missing; prepare the managed environment explicitly"
        ) from exc

    if not isinstance(result, (pl.DataFrame, pl.LazyFrame)):
        raise DeclarationError(
            "Polars transform must return one DataFrame or LazyFrame; got " + type(result).__name__
        )
