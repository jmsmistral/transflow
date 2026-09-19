//! Portable logical schemas; types may exist even when an engine cannot execute them.

use crate::{
    DomainError, ErrorKind,
    value::{DecimalType, FloatWidth, IntegerType, TimestampType},
};
use std::collections::{BTreeMap, BTreeSet};

/// Nonempty field name of at most 256 Unicode scalar values, excluding ASCII controls.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FieldName(String);
impl FieldName {
    /// Preserve Unicode/case exactly; no normalization or identifier coercion.
    pub fn new(text: impl Into<String>) -> Result<Self, DomainError> {
        let text = text.into();
        if text.is_empty()
            || text.chars().count() > 256
            || text.chars().any(|c| c.is_ascii_control())
        {
            return Err(DomainError::new(ErrorKind::InvalidName));
        }
        Ok(Self(text))
    }
    /// Original validated name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
/// Engine-independent type, distinct from any materialized value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LogicalType {
    /// Boolean.
    Bool,
    /// Signed or unsigned integer with explicit width.
    Integer(IntegerType),
    /// IEEE float with explicit width.
    Float(FloatWidth),
    /// Unicode string.
    String,
    /// Binary bytes.
    Binary,
    /// Gregorian date.
    Date,
    /// Validated decimal precision/scale.
    Decimal(DecimalType),
    /// Explicit timestamp resolution and optional timezone.
    Timestamp(TimestampType),
    /// Element field includes its own name, nullability and metadata.
    List(Box<Field>),
    /// Uniquely named ordered fields.
    Struct(Fields),
}
/// Field construction requires an already validated name and logical type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Field {
    name: FieldName,
    logical_type: LogicalType,
    nullable: bool,
    metadata: Option<BTreeMap<String, String>>,
}
impl Field {
    /// Preserve optional metadata (including absent versus an explicitly empty map).
    pub fn new(
        name: FieldName,
        logical_type: LogicalType,
        nullable: bool,
        metadata: Option<BTreeMap<String, String>>,
    ) -> Self {
        Self {
            name,
            logical_type,
            nullable,
            metadata,
        }
    }
    /// Validated name.
    pub fn name(&self) -> &FieldName {
        &self.name
    }
    /// Declared type; it does not imply support by a particular adapter.
    pub fn logical_type(&self) -> &LogicalType {
        &self.logical_type
    }
    /// Whether null is allowed for this field.
    pub fn nullable(&self) -> bool {
        self.nullable
    }
    /// Non-executable field annotations, preserving optional presence.
    pub fn metadata(&self) -> Option<&BTreeMap<String, String>> {
        self.metadata.as_ref()
    }
}
/// An ordered field collection with uniqueness enforced at construction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fields(Vec<Field>);
impl Fields {
    /// Duplicate names fail at the second occurrence; empty schemas are representable.
    pub fn new(fields: Vec<Field>) -> Result<Self, DomainError> {
        let mut names = BTreeSet::new();
        for (i, field) in fields.iter().enumerate() {
            if !names.insert(field.name().as_str()) {
                return Err(DomainError::new(ErrorKind::DuplicateName)
                    .at("name")
                    .at(i.to_string())
                    .at("fields"));
            }
        }
        Ok(Self(fields))
    }
    /// Original field order, without mutable access that could invalidate uniqueness.
    pub fn as_slice(&self) -> &[Field] {
        &self.0
    }
}
/// LogicalSchemaV1 domain payload; its wire format version belongs to tf-protocol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogicalSchema(Fields);
impl LogicalSchema {
    /// Construct an ordered schema with unique column names.
    pub fn new(fields: Vec<Field>) -> Result<Self, DomainError> {
        Fields::new(fields).map(Self)
    }
    /// Validated, immutable columns.
    pub fn fields(&self) -> &[Field] {
        self.0.as_slice()
    }
}
