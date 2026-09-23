# Core expectation DSL (T059–T060)

An expectation is immutable syntax. Constructing it, attaching it to a `Check`,
discovering it or running `transflow validate` never evaluates rows or invokes a
producer. T060 supplies pure Rust truth/metric kernels; artifact evaluation and
publication gates remain T061–T062. Successful workspace validation is not a
data-quality PASS.

The SDK distinguishes `RowPredicate`, `DatasetExpectation`, `ColumnRef`, `Literal`,
`ScalarMetric` and `InputAliasRef`. `Check` accepts only the first two. Literals use
the existing lossless `ScalarValue` carrier, including integer width, decimal scale
and timestamp metadata. Engine expressions, arbitrary objects, closures and raw
SQL are not valid AST nodes. Column names remain exact data, even when they contain
quotes or SQL-like text; a later compiler must quote identifiers.

```python
from transflow import Check
from transflow import expectations as E

present = E.col("age").non_null()
valid_age = E.all(present, E.col("age").gte(0), E.col("age").lt(200))
check = Check(valid_age, "Age policy").effective(null_policy="fail", sample_rows=0)
combined = E.all(E.primary_key("id"), E.every(present))
nonempty = Check(E.row_count().gt(0), "At least one row")
allowed = Check(E.col("status").is_in("active", "paused", None), "Known status")
schema = Check(E.col("age").has_type("i64"), "Integer age")
```

The G1 DSL supplies `col().non_null()`, `.is_null()`, `.is_in(*values)`, `.is_finite()`,
`.is_nan()`, `.exists()`, `.has_type(type)`, `primary_key`, explicit `literal`/`compare`,
`row_count`, `all`, `any`, `not_` and `every` syntax.
Comparison operators are `gt`, `gte`, `lt`, `lte`, `equals` and `not_equals`.
Column comparisons require a column operand; metric comparisons require a metric
and accept only integer count literals or another row-count metric. No row/scalar
mix or implicit string-to-number conversion is accepted. Null comparisons require
explicit null predicates. All six comparisons have column and row-count convenience
methods. `input(alias).row_count()` is representable syntax; cross-input metric
evaluation remains G3. Representable syntax is not evidence of an available evaluator.

Convenience literals accept Python bool, int, float and str, plus None for membership.
Integers use i64 (or u64 above i64's maximum); floats use f64. Other types and widths
require `E.literal` with an explicit ScalarValue carrier. Overflow and incompatible
types fail instead of coercing. `has_type` accepts a primitive canonical name such as
`"i64"` or a complete LogicalType dictionary, including decimal/timestamp metadata;
it does not accept an engine dtype. Empty membership is always false.

Boolean operands must share a kind. Promote a row predicate with `every` to combine
it with a dataset expectation. Empty combinations, implicit Python `and`/`or` truth
conversion, unknown operators/fields/versions and duplicate primary-key names fail.
`not_` preserves kind. The root Check's effective null policy applies to the complete
predicate; constructors do not interpret null truth values. Inherited null/sample
policies are resolved before compute fingerprinting, preserving explicit fail/zero.

`to_wire()` returns a detached AST-v1 dictionary; `Expectation.from_wire()` validates
and freezes it. Roots carry `ast_version: 1`, children carry closed operator tags.
The authored schema generates Python assertions and TypeScript types. Rust domain
syntax has separate row, dataset, column and scalar enums; `tf_protocol::expectation`
decodes and re-encodes the same representation. Shared fixtures compare canonical
bytes and effective-check Compute digests without an engine.

New discovery/execute workers require `expectation.ast.v1` and `expectation.core.v1`
alongside their operation capability. Receivers lacking either reject the frames. The catalogue still
accepts the original unversioned non-null/primary-key seed and upgrades it to AST-v1
before fingerprinting. Other unversioned nodes are rejected. Retained validation
certificates use a new check-semantics marker, so old evidence cannot authorize the
new semantics. Protocol 1.0 and the already reserved AST version 1 remain unchanged.

The whole-workspace validator rejects undeclared input aliases, including references
nested in dataset expressions. Data and validation-only aliases are both eligible;
no reference opens a catalogue or reads a moving head. With an available declared
schema, Rust checks nested column names, compatible comparison types and supported
primary-key types. Integer widths may be compared exactly; other logical types and
metadata must match. Foreign, named-branch and unknown future schemas retain deferred
obligations. Actual pinned/candidate schema and evaluator capability checks remain
mandatory later, even if declared-schema preflight succeeds.

The [pure domain kernels](../crates/tf-domain/src/expectation/semantics.rs) establish
SQL three-valued composition before applying the complete row predicate's null
policy. `RowMetrics.rows_evaluated` counts every inspected row, including unknown
rows counted separately as skipped under `ignore`. False always violates. Null
membership matches only explicit None. NaN is non-null, compares false except for
`not_equals`, and does not match itself in membership. Positive and negative zero
compare equal. Finiteness excludes NaN and both infinities; float predicates on null
return unknown. Missing columns and incompatible types are errors even under WARN;
`exists` deliberately permits a missing column as a normal false outcome. A present
column with a different `has_type` is also a normal false outcome.

Primary keys admit only bool, integers, string, decimal, date and timestamp. They
preserve tuple boundaries and exact scalar precision. Any null component counts
one null-key row, excluded from duplicate grouping. For `[1, 2, 2, null]`, metrics are
`null_key_rows=1`, `duplicate_groups=1`, `duplicate_group_rows=2`, `violating_rows=3`.
Every row in a duplicate group counts. Empty row/key checks pass; `row_count().gt(0)`
fails on zero. These rules have [shared semantic fixtures](../schemas/fixtures/expectation-semantics-v1.json).
The key reducer consumes externally grouped counts and holds no dataset-sized key
map. T061 still owns real Parquet scans, exact grouping/spill and error reporting;
T062 owns lifecycle gates. No public build check execution is delivered here.

Expression processing is bounded to 1,000 nodes and at most 32 semantic levels;
the existing stricter enclosing schema/transport depth and byte limits also apply.
Recursive schema alternatives check discriminators before children, independent
of JSON key order. No dependency, database, runtime memory policy or engine pin
changes are introduced by this task.
