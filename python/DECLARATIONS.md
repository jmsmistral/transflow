# Python authoring declarations (T026)

## Declaration layout

Put the Output first and each Input below it on a separate line. Place checks
below the corresponding binding in a `checks=[...]` block, one Check per line.
Use trailing commas to retain this layout, even for one binding or one check.
The same convention applies to Python, metadata-aware Python, SQL and source
transforms, including generated sample workspaces. Keyword order is a readability
convention; existing declarations remain valid.

```python
from transflow import Check, Input, Output, transform
from transflow import expectations as E


@transform(
    output=Output(
        "curated/orders",
        checks=[
            Check(E.primary_key("id"), "Unique order IDs"),
            Check(E.row_count().gt(0), "At least one order"),
        ],
    ),
    orders=Input(
        "raw/orders",
        checks=[
            Check(E.col("amount").gte(0), "Nonnegative amounts"),
        ],
    ),
)
def checked_orders(orders):
    return orders
```

## Declarations and binding

`Input`, `Output`, `Check`, `transform`, `transform_meta`, `sql_transform` and `source_transform` are available from
`transflow`. Decorators return the original function, preserving its signature,
annotations and normal direct-call behaviour. They attach a frozen `Declaration`,
available through `transflow.declarations.get_declaration(function)`; ordinary
helpers return `None`. Declaration construction never opens a catalogue, resolves a
branch head, installs packages, executes checks or starts a worker.

```python
from transflow import Branch, Check, Input, Output, transform
from transflow import expectations as E


@transform(
    output=Output(
        "curated/items",
        checks=[
            Check(E.primary_key("id"), "Unique item IDs", id="item_pk"),
        ],
    ),
    items=Input("raw/items", branch=Branch.CURRENT),
)
def curated(items):
    return items
```

Omitted branches, `Branch.CURRENT` and literal branch names remain distinct.
Only `stop_branch_fallback=True` disables fallback. `Input(version=...)` and unknown
keyword aliases fail. String references can be paths or `dataset:UUID`; bound `C`
references retain their own immutable identity. Catalogue binding uses a verified immutable snapshot; explicit test contexts
remain separate from worker execution. Registration and
foreign-ownership resolution remain coordinator responsibilities.

Checks normalize to tuples, reject duplicate IDs within each binding, and retain
`None` for inherited null/sample policy. `check.effective(null_policy="ignore",
sample_rows=3)` produces a new check with explicit effective values before later
fingerprinting. Explicit `fail` or zero wins over workspace settings. Sample counts
are diagnostic limits, never permission to sample required check evaluation.
Only `FAIL` and `WARN` severities are valid. Names without explicit IDs use a
case-folded word slug with underscore separators; names with no word characters need
an explicit ID.

The [T059–T060 expectation DSL](EXPECTATIONS.md) extends primary-key/non-null declarations
with typed comparisons, boolean composition, explicit promotion and input references.
Discovery emits versioned canonical syntax; workspace validation resolves policies
and checks structural semantics. G1 column/metric convenience methods, membership,
null/float/schema predicates and exact key/truth kernels are available. Artifact
evaluation and managed-build input/output gates are implemented by the
[canonical check service](../crates/tf-exec/CHECKS.md).
Lambdas, SQL strings and engine expressions are not accepted as expectations.

With `@transform`, exactly the `role="data"` aliases are lazy dataframe parameters.
`@transform_meta` receives pinned handles and requires `.dataframe()` to read them;
function-based `@sql_transform` supplies handles for the arguments it declares and
may omit input arguments altogether. `role="validation"` inputs remain in metadata
but do not appear in the signature. Optional `ctx` must be
keyword-only; annotate it `TransformContext` for type checking. Runtime signature
validation inspects call mechanics without resolving annotation types. The worker injects the frozen execution context; ordinary direct calls
remain ordinary Python calls. Annotations are preserved on the original callable without evaluating imports,
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
these declarations through `params={name: Parameter(...)}`; plan overrides are resolved before execution. Nested schema/default/lineage input mutations cannot change retained
metadata. These copied JSON bytes are storage, not computation fingerprints.

Direct function calls are ordinary Python, including their return values.
`transflow.declarations.validate_result(function, result)` is the explicit boundary
that accepts only a real Polars DataFrame or LazyFrame, without collecting it.
Missing Polars is an actionable environment error; nothing installs implicitly.
T058 [materializes Polars results](POLARS.md) once through the private worker;
required checks and guarded publication compose around that materialization.

Verification combines pure declarations, installed-wheel import/type/package tests,
and `python/tools/check_declaration_returns.py` against pinned real Polars in all
three native qualification jobs. No new dependency is required by importing the SDK.

Metadata-aware handles and explicit multi-producer exports are described in
[Dataset handles and transform factories](TRANSFORM_FACTORIES.md). `transform`
keeps its direct lazy dataframe arguments; SQL functions use handles by default.
