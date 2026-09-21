# Typed expectation AST (T059)

An expectation is immutable syntax. Constructing it, attaching it to a `Check`,
discovering it or running `transflow validate` never evaluates rows or invokes a
producer. Actual truth tables, metrics, evaluation and publication gates remain
T060–T062. Successful workspace validation is not a data-quality PASS.

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
minimum = E.compare(E.col("age"), "gte", E.literal({"type": "i64", "value": "0"}))
valid_age = E.all(present, minimum)
check = Check(valid_age, "Age policy").effective(null_policy="fail", sample_rows=0)
combined = E.all(E.primary_key("id"), E.every(present))
```

T059 supplies `col().non_null()`, `primary_key`, explicit `literal`/`compare`,
`row_count`, `input(alias).row_count`, `all`, `any`, `not_` and `every` syntax.
Comparison operators are `gt`, `gte`, `lt`, `lte`, `equals` and `not_equals`.
Column comparisons require a column operand; metric comparisons require a metric
and accept only integer count literals or another row-count metric. No row/scalar
mix or implicit string-to-number conversion is accepted. Null comparisons require
explicit null predicates. Convenience column/metric methods and the remaining G1
operators belong to T060; cross-input metric evaluation remains G3. Representable
syntax is not evidence of an available evaluator.

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

New discovery/execute workers require `expectation.ast.v1` alongside their operation
capability. Receivers lacking that feature reject the frames. The catalogue still
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

Expression processing is bounded to 1,000 nodes and at most 32 semantic levels;
the existing stricter enclosing schema/transport depth and byte limits also apply.
Recursive schema alternatives check discriminators before children, independent
of JSON key order. No dependency, database, runtime memory policy or engine pin
changes are introduced by this task.
