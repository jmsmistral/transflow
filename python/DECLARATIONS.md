# Python authoring declarations (T026)

`Input`, `Output`, `Check`, `transform` and `source_transform` are available from
`transflow`. Decorators return the original function, preserving its signature,
annotations and normal direct-call behaviour. They attach a frozen `Declaration`,
available through `transflow.declarations.get_declaration(function)`; ordinary
helpers return `None`. Declaration construction never opens a catalogue, resolves a
branch head, installs packages, executes checks or starts a worker.

```python
from transflow import Branch, Check, Input, Output, transform
from transflow import expectations as E


@transform(
    items=Input("raw/items", branch=Branch.CURRENT),
    output=Output(
        "curated/items", checks=Check(E.primary_key("id"), "Unique item IDs", id="item_pk")
    ),
)
def curated(items):
    return items
```

Omitted branches, `Branch.CURRENT` and literal branch names remain distinct.
Only `stop_branch_fallback=True` disables fallback. `Input(version=...)` and unknown
keyword aliases fail. String references can be paths or `dataset:UUID`; bound `C`
references retain their own immutable identity. Production catalogue binding remains
T030; existing explicit test contexts work with declarations today. Registration and
foreign-ownership resolution remain coordinator responsibilities.

Checks normalize to tuples, reject duplicate IDs within each binding, and retain
`None` for inherited null/sample policy. `check.effective(null_policy="ignore",
sample_rows=3)` produces a new check with explicit effective values before later
fingerprinting. Explicit `fail` or zero wins over workspace settings. Sample counts
are diagnostic limits, never permission to sample required check evaluation.
Only `FAIL` and `WARN` severities are valid. Names without explicit IDs use a
case-folded word slug with underscore separators; names with no word characters need
an explicit ID.

The [T059 expectation AST](EXPECTATIONS.md) extends primary-key/non-null declarations
with typed comparisons, boolean composition, explicit promotion and input references.
Discovery emits versioned canonical syntax; workspace validation resolves policies
and checks structural semantics. The remaining DSL and evaluation are T060–T062.
Lambdas, SQL strings and engine expressions are not accepted as expectations.

Exactly the `role="data"` aliases are dataframe parameters. `role="validation"`
inputs remain in metadata but do not appear in the signature. Optional `ctx` must be
keyword-only; annotate it `TransformContext` for type checking. Runtime signature
validation inspects call mechanics without resolving annotation types. The context is a typed interface; actual execution-owned
context injection is later work. Annotations are preserved on the original callable without evaluating imports,
forward references or Python 3.14 deferred annotation functions.

`source_transform` has one mandatory Output and no dataframe inputs. Its refresh
policy is `always`, `manual` or `{"ttl_seconds": positive_integer}`. Secret references
are names only. `transform` cache policy is `deterministic` or `never`.
`resources={"wall_timeout_seconds": 0}` explicitly disables the transform deadline;
positive integer seconds override it. Other resource keys fail rather than being
silently ignored. There is no default memory limit.

`Output(schema=...)` validates and copies the versioned logical-schema contract.
`Parameter(logical_type, default)` uses the existing lossless `LogicalType` and
`WireValue` carriers, including tagged integers/decimals/timestamps. Explicit list
element fields describe nullability; fixed-key typed maps use the existing struct
fields carrier. Nonfinite floats and mismatched defaults fail. A transform receives
these declarations through `params={name: Parameter(...)}`; plan override resolution
is later work. Nested schema/default/lineage input mutations cannot change retained
metadata. These copied JSON bytes are storage, not computation fingerprints.

Direct function calls are ordinary Python, including their return values.
`transflow.declarations.validate_result(function, result)` is the explicit boundary
that accepts only a real Polars DataFrame or LazyFrame, without collecting it.
Missing Polars is an actionable environment error; nothing installs implicitly.
T058 [materializes Polars results](POLARS.md) once through the private worker;
canonical checks and full runtime/publication composition remain later tasks.

Verification combines pure declarations, installed-wheel import/type/package tests,
and `python/tools/check_declaration_returns.py` against pinned real Polars in all
three native qualification jobs. No new dependency is required by importing the SDK.
