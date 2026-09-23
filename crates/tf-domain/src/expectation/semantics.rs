//! Exact G1 semantics over validated scalars and already grouped key counts.
//! No file scans, SQL, scheduling, sampling or dataset-sized row buffers.
use super::Comparison;
use crate::{schema::LogicalType, value::ScalarValue};
use std::cmp::Ordering;

/// Unknown is retained until the complete predicate reaches its root policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Truth {
    /// Satisfied.
    True,
    /// Violated.
    False,
    /// Null-dependent result.
    Unknown,
}
impl Truth {
    /// SQL three-valued conjunction.
    pub fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            _ => Self::True,
        }
    }
    /// SQL three-valued disjunction.
    pub fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            _ => Self::False,
        }
    }
    /// Explicit conversion of a non-null boolean.
    pub fn from_bool(value: bool) -> Self {
        if value { Self::True } else { Self::False }
    }
}
impl std::ops::Not for Truth {
    type Output = Self;
    fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }
}
/// Apply only at the complete row predicate, including one explicitly promoted with every.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NullPolicy {
    /// Unknown contributes a violation.
    Fail,
    /// Unknown is skipped, never relabelled true.
    Ignore,
}
/// Typed evaluation errors are distinct from a data violation, even under WARN.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticError {
    /// Incompatible operands or unsupported key type.
    Type,
    /// Exact metrics exceeded their u64 carrier.
    Overflow,
    /// Invalid group count or tuple arity.
    Shape,
}
impl std::fmt::Display for SemanticError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Type => "Expectation operand types are incompatible or unsupported",
            Self::Overflow => "Expectation count exceeds the exact metric range",
            Self::Shape => "Expectation tuple or group shape is invalid",
        })
    }
}
impl std::error::Error for SemanticError {}
/// Null validity never classifies NaN as missing.
pub fn is_null(value: &ScalarValue) -> Truth {
    Truth::from_bool(matches!(value, ScalarValue::Null))
}
/// Float predicate; callers preflight the schema even for null/empty data.
pub fn is_finite(value: &ScalarValue) -> Result<Truth, SemanticError> {
    match value {
        ScalarValue::Null => Ok(Truth::Unknown),
        ScalarValue::Float(f) => Ok(Truth::from_bool(f.as_f64().is_finite())),
        _ => Err(SemanticError::Type),
    }
}
/// Float predicate; null is unknown and NaN is distinct from null.
pub fn is_nan(value: &ScalarValue) -> Result<Truth, SemanticError> {
    match value {
        ScalarValue::Null => Ok(Truth::Unknown),
        ScalarValue::Float(f) => Ok(Truth::from_bool(f.as_f64().is_nan())),
        _ => Err(SemanticError::Type),
    }
}
/// Compare validated scalars without a float intermediary for integers/decimals/timestamps.
/// NaN makes every comparison false except not-equals. Null yields unknown.
pub fn compare(
    op: Comparison,
    left: &ScalarValue,
    right: &ScalarValue,
) -> Result<Truth, SemanticError> {
    use ScalarValue as V;
    if matches!(left, V::Null) || matches!(right, V::Null) {
        return Ok(Truth::Unknown);
    }
    let order = match (left, right) {
        (V::Integer(a), V::Integer(b)) => Some(a.as_i128().cmp(&b.as_i128())),
        (V::Float(a), V::Float(b)) if a.width() == b.width() => a.as_f64().partial_cmp(&b.as_f64()),
        (V::Decimal(a), V::Decimal(b)) if a.kind() == b.kind() => {
            Some(a.coefficient().cmp(&b.coefficient()))
        }
        (V::Timestamp(a), V::Timestamp(b)) if a.kind() == b.kind() => {
            Some(a.as_str().cmp(b.as_str()))
        }
        (V::Date(a), V::Date(b)) => Some(a.as_str().cmp(b.as_str())),
        (V::Bool(a), V::Bool(b)) => Some(a.cmp(b)),
        (V::String(a), V::String(b)) => Some(a.cmp(b)),
        (V::Binary(a), V::Binary(b)) => Some(a.cmp(b)),
        _ => return Err(SemanticError::Type),
    };
    Ok(Truth::from_bool(match op {
        Comparison::Gt => order == Some(Ordering::Greater),
        Comparison::Gte => matches!(order, Some(Ordering::Greater | Ordering::Equal)),
        Comparison::Lt => order == Some(Ordering::Less),
        Comparison::Lte => matches!(order, Some(Ordering::Less | Ordering::Equal)),
        Comparison::Equals => order == Some(Ordering::Equal),
        Comparison::NotEquals => order != Some(Ordering::Equal),
    }))
}
/// Exact membership; null matches only an explicit null. Incompatible alternatives error.
pub fn is_in(value: &ScalarValue, allowed: &[ScalarValue]) -> Result<Truth, SemanticError> {
    let mut found = false;
    for item in allowed {
        let equal = match (value, item) {
            (ScalarValue::Null, ScalarValue::Null) => true,
            (ScalarValue::Null, _) | (_, ScalarValue::Null) => false,
            _ => compare(Comparison::Equals, value, item)? == Truth::True,
        };
        found |= equal;
    }
    Ok(Truth::from_bool(found))
}
fn plus(a: u64, b: u64) -> Result<u64, SemanticError> {
    a.checked_add(b).ok_or(SemanticError::Overflow)
}
/// Incremental exact root metrics. Empty input starts with zero counts and passes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RowMetrics {
    /// All rows examined, including skipped unknowns.
    pub rows_evaluated: u64,
    /// False rows plus unknowns under fail policy.
    pub violating_rows: u64,
    /// Unknown rows excluded by ignore policy.
    pub skipped_rows: u64,
}
impl RowMetrics {
    /// Record a complete predicate result. Overflow leaves the accumulator unchanged.
    pub fn observe(&mut self, truth: Truth, policy: NullPolicy) -> Result<(), SemanticError> {
        let skip = truth == Truth::Unknown && policy == NullPolicy::Ignore;
        let violation =
            truth == Truth::False || (truth == Truth::Unknown && policy == NullPolicy::Fail);
        *self = Self {
            rows_evaluated: plus(self.rows_evaluated, 1)?,
            violating_rows: plus(self.violating_rows, u64::from(violation))?,
            skipped_rows: plus(self.skipped_rows, u64::from(skip))?,
        };
        Ok(())
    }
    /// Data outcome only; schema/compiler/transport failures must be handled separately.
    pub fn passes(&self) -> bool {
        self.violating_rows == 0
    }
}
/// An exact scalar key component. Type agreement is checked by KeySchema before creation.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum KeyAtom {
    /// Boolean key.
    Bool(bool),
    /// Exact integer or normalized decimal coefficient, with schema-bound meaning.
    Number(i128),
    /// Exact string/date/timestamp spelling, with schema-bound meaning.
    Text(String),
}
/// Validated ordered key types. Reuse for every row in one dataset; no row storage.
#[derive(Clone, Debug)]
pub struct KeySchema(Vec<LogicalType>);
impl KeySchema {
    /// Reject unsupported types before seeing any rows, including empty/all-null tables.
    pub fn new(types: Vec<LogicalType>) -> Result<Self, SemanticError> {
        if types.is_empty() {
            return Err(SemanticError::Shape);
        }
        if types.iter().any(|t| {
            !matches!(
                t,
                LogicalType::Bool
                    | LogicalType::Integer(_)
                    | LogicalType::String
                    | LogicalType::Decimal(_)
                    | LogicalType::Date
                    | LogicalType::Timestamp(_)
            )
        }) {
            return Err(SemanticError::Type);
        }
        Ok(Self(types))
    }
    /// None denotes one null-key row, excluded from grouping. Other rows retain tuple boundaries.
    /// Validate every component even if another is null; errors must not be hidden.
    pub fn tuple(&self, values: &[ScalarValue]) -> Result<Option<Vec<KeyAtom>>, SemanticError> {
        if values.len() != self.0.len() {
            return Err(SemanticError::Shape);
        }
        let mut result = Vec::with_capacity(values.len());
        let mut null = false;
        for (typ, value) in self.0.iter().zip(values) {
            use ScalarValue as V;
            let atom = match (typ, value) {
                (_, V::Null) => {
                    null = true;
                    continue;
                }
                (LogicalType::Bool, V::Bool(v)) => KeyAtom::Bool(*v),
                (LogicalType::Integer(t), V::Integer(v)) if *t == v.kind() => {
                    KeyAtom::Number(v.as_i128())
                }
                (LogicalType::Decimal(t), V::Decimal(v)) if *t == v.kind() => {
                    KeyAtom::Number(v.coefficient())
                }
                (LogicalType::String, V::String(v)) => KeyAtom::Text(v.clone()),
                (LogicalType::Date, V::Date(v)) => KeyAtom::Text(v.as_str().into()),
                (LogicalType::Timestamp(t), V::Timestamp(v)) if t == v.kind() => {
                    KeyAtom::Text(v.as_str().into())
                }
                _ => return Err(SemanticError::Type),
            };
            result.push(atom);
        }
        Ok(if null { None } else { Some(result) })
    }
}
/// Reducer for exact external GROUP BY results; it never retains the dataset or a key map.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PrimaryKeyMetrics {
    /// Rows with one or more null components, counted once.
    pub null_key_rows: u64,
    /// Non-null tuple groups containing at least two rows.
    pub duplicate_groups: u64,
    /// All rows in duplicate groups, not just the excess rows.
    pub duplicate_group_rows: u64,
    /// Null rows plus duplicate-group rows (disjoint sets).
    pub violating_rows: u64,
}
impl PrimaryKeyMetrics {
    /// Add the exact null-row count, independently of the root null policy.
    pub fn add_null_rows(&mut self, count: u64) -> Result<(), SemanticError> {
        let updated = Self {
            null_key_rows: plus(self.null_key_rows, count)?,
            violating_rows: plus(self.violating_rows, count)?,
            ..*self
        };
        *self = updated;
        Ok(())
    }
    /// Consume each distinct non-null tuple group exactly once. Zero is malformed.
    pub fn add_group(&mut self, count: u64) -> Result<(), SemanticError> {
        if count == 0 {
            return Err(SemanticError::Shape);
        }
        if count > 1 {
            *self = Self {
                duplicate_groups: plus(self.duplicate_groups, 1)?,
                duplicate_group_rows: plus(self.duplicate_group_rows, count)?,
                violating_rows: plus(self.violating_rows, count)?,
                ..*self
            };
        }
        Ok(())
    }
    /// An empty primary key check passes; nonempty data requires a separate row count condition.
    pub fn passes(&self) -> bool {
        self.violating_rows == 0
    }
}
