//! Pure expectation syntax. Constructors/decoders validate structure before evaluation.
//! Literal carriers are supplied by a versioned codec; no engine objects or closures.
use crate::schema::FieldName;

/// Explicit comparison; no SQL snippets or implicit coercion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Comparison {
    /// Strictly greater.
    Gt,
    /// Greater or equal.
    Gte,
    /// Strictly less.
    Lt,
    /// Less or equal.
    Lte,
    /// Equal under canonical typed semantics.
    Equals,
    /// Not equal under canonical typed semantics.
    NotEquals,
}
/// Nonempty boolean combinations are enforced at the decoding boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BooleanOp {
    /// All operands must hold.
    All,
    /// At least one operand must hold.
    Any,
}
/// A declared input alias, never a dataset path or latest-head lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputAliasRef(pub FieldName);
/// Exact scalar aggregates. Additional metrics require qualified semantics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScalarMetric {
    /// Count rows in the current dataset or a declared input.
    RowCount(Option<InputAliasRef>),
}
/// Per-row value, distinct from scalar aggregate operands.
#[derive(Clone, Debug, PartialEq)]
pub enum ColumnValue<L> {
    /// Named column from the current checked dataset.
    Column(FieldName),
    /// Explicit lossless constant.
    Literal(L),
}
/// Dataset scalar, never a per-row column.
#[derive(Clone, Debug, PartialEq)]
pub enum ScalarOperand<L> {
    /// Whole-dataset scalar aggregate.
    Metric(ScalarMetric),
    /// Explicit lossless constant.
    Literal(L),
}
/// Per-row three-valued predicate syntax, without evaluation.
#[derive(Clone, Debug, PartialEq)]
pub enum RowPredicate<L> {
    /// Column value is present.
    NonNull(FieldName),
    /// Null is explicitly present.
    IsNull(FieldName),
    /// Float is finite; null produces unknown.
    IsFinite(FieldName),
    /// Float is NaN; null produces unknown.
    IsNan(FieldName),
    /// Exact allowed literals, with explicit null membership.
    IsIn(FieldName, Vec<L>),
    /// Explicit comparison of compatible typed operands.
    Compare(Comparison, ColumnValue<L>, ColumnValue<L>),
    /// Composition of the same boolean kind.
    Boolean(BooleanOp, Vec<Self>),
    /// Negation retaining the expression kind.
    Not(Box<Self>),
}
/// Whole-dataset outcome syntax; promotion is explicit.
#[derive(Clone, Debug, PartialEq)]
pub enum DatasetExpectation<L> {
    /// Non-null unique tuples across the named columns.
    PrimaryKey(Vec<FieldName>),
    /// Schema existence; a missing column is a normal violation.
    Exists(FieldName),
    /// Exact logical type test; the codec validates the type payload.
    HasType(FieldName, L),
    /// Explicit comparison of compatible typed operands.
    Compare(Comparison, ScalarOperand<L>, ScalarOperand<L>),
    /// Explicitly require a row predicate across the dataset.
    Every(RowPredicate<L>),
    /// Composition of the same boolean kind.
    Boolean(BooleanOp, Vec<Self>),
    /// Negation retaining the expression kind.
    Not(Box<Self>),
}
/// Only boolean expression kinds can be attached to a Check.
#[derive(Clone, Debug, PartialEq)]
pub enum Expectation<L> {
    /// A predicate implicitly required across all non-ignored rows.
    Row(RowPredicate<L>),
    /// A whole-dataset condition.
    Dataset(DatasetExpectation<L>),
}

/// Pure truth, scalar and exact key metric semantics; no artifact scans.
pub mod semantics;
