//! Engine-independent values. Text carriers preserve precision and declared metadata.

use crate::{DomainError, ErrorKind, schema::FieldName};
use std::collections::BTreeSet;

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit())
}
fn unsigned(s: &str) -> bool {
    digits(s) && (s == "0" || !s.starts_with('0'))
}
fn decimal_syntax(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    match s.split_once('.') {
        Some((whole, fraction)) => unsigned(whole) && digits(fraction),
        None => unsigned(s),
    }
}
fn float_syntax(s: &str) -> bool {
    let mut parts = s.split(['e', 'E']);
    let mantissa = parts.next().is_some_and(decimal_syntax);
    let exponent = parts
        .next()
        .is_none_or(|s| digits(s.strip_prefix(['+', '-']).unwrap_or(s)));
    mantissa && exponent && parts.next().is_none()
}

/// Integer width is part of the logical value, even when magnitudes coincide.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegerType {
    /// Signed 8-bit.
    I8,
    /// Signed 16-bit.
    I16,
    /// Signed 32-bit.
    I32,
    /// Signed 64-bit.
    I64,
    /// Unsigned 8-bit.
    U8,
    /// Unsigned 16-bit.
    U16,
    /// Unsigned 32-bit.
    U32,
    /// Unsigned 64-bit.
    U64,
}
/// A checked integer; the private magnitude fits its declared type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntegerValue {
    kind: IntegerType,
    value: i128,
}
impl IntegerValue {
    /// Parse canonical decimal text without routing through floating point.
    pub fn parse(kind: IntegerType, text: &str) -> Result<Self, DomainError> {
        let unsigned_text = text.strip_prefix('-').unwrap_or(text);
        if !unsigned(unsigned_text) || text == "-0" {
            return Err(DomainError::new(ErrorKind::InvalidNumber));
        }
        let value = text
            .parse::<i128>()
            .map_err(|_| DomainError::new(ErrorKind::OutOfRange))?;
        let (min, max) = match kind {
            IntegerType::I8 => (i8::MIN as i128, i8::MAX as i128),
            IntegerType::I16 => (i16::MIN as i128, i16::MAX as i128),
            IntegerType::I32 => (i32::MIN as i128, i32::MAX as i128),
            IntegerType::I64 => (i64::MIN as i128, i64::MAX as i128),
            IntegerType::U8 => (0, u8::MAX as i128),
            IntegerType::U16 => (0, u16::MAX as i128),
            IntegerType::U32 => (0, u32::MAX as i128),
            IntegerType::U64 => (0, u64::MAX as i128),
        };
        if value < min || value > max {
            return Err(DomainError::new(ErrorKind::OutOfRange));
        }
        Ok(Self { kind, value })
    }
    /// Declared signedness and width.
    pub fn kind(self) -> IntegerType {
        self.kind
    }
    /// Exact widened integer, without precision loss.
    pub fn as_i128(self) -> i128 {
        self.value
    }
    /// Canonical decimal carrier.
    pub fn to_text(self) -> String {
        self.value.to_string()
    }
}
/// IEEE width, independent of the carrier string.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FloatWidth {
    /// IEEE binary32.
    F32,
    /// IEEE binary64.
    F64,
}
/// Validated float text and bits; equality compares representations, not SQL semantics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FloatValue {
    width: FloatWidth,
    text: String,
    bits: u64,
}
impl FloatValue {
    /// Preserve text while rejecting finite overflow and nonzero-to-zero underflow.
    pub fn parse(width: FloatWidth, text: &str) -> Result<Self, DomainError> {
        let special = ["NaN", "Infinity", "-Infinity"].contains(&text);
        if !special && !float_syntax(text) {
            return Err(DomainError::new(ErrorKind::InvalidNumber));
        }
        let value = match text {
            "NaN" => f64::NAN,
            "Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            _ => text
                .parse::<f64>()
                .map_err(|_| DomainError::new(ErrorKind::InvalidNumber))?,
        };
        let nonzero = text
            .split(['e', 'E'])
            .next()
            .is_some_and(|s| s.bytes().any(|c| matches!(c, b'1'..=b'9')));
        let rounded = match width {
            FloatWidth::F32 => f64::from(value as f32),
            FloatWidth::F64 => value,
        };
        if !special && (!rounded.is_finite() || (rounded == 0.0 && nonzero)) {
            return Err(DomainError::new(ErrorKind::OutOfRange));
        }
        Ok(Self {
            width,
            text: text.to_owned(),
            bits: rounded.to_bits(),
        })
    }
    /// Exact source carrier; canonical hashing spelling is a separate contract.
    pub fn as_str(&self) -> &str {
        &self.text
    }
    /// Original IEEE width.
    pub fn width(&self) -> FloatWidth {
        self.width
    }
    /// Native value; binary32 widens exactly to binary64, retaining signed zero.
    pub fn as_f64(&self) -> f64 {
        f64::from_bits(self.bits)
    }
}
/// Decimal128 precision and scale, with no implicit float conversion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecimalType {
    precision: u8,
    scale: i8,
}
impl DecimalType {
    /// Validate precision 1–38 and scale −38–38; scale may exceed precision.
    pub fn new(precision: u8, scale: i8) -> Result<Self, DomainError> {
        if !(1..=38).contains(&precision) {
            return Err(DomainError::new(ErrorKind::InvalidDecimalType).at("precision"));
        }
        if !(-38..=38).contains(&scale) {
            return Err(DomainError::new(ErrorKind::InvalidDecimalType).at("scale"));
        }
        Ok(Self { precision, scale })
    }
    /// Maximum number of significant unscaled digits.
    pub fn precision(self) -> u8 {
        self.precision
    }
    /// Power of ten dividing the unscaled coefficient.
    pub fn scale(self) -> i8 {
        self.scale
    }
}
/// Exact coefficient plus original fixed-point text, including scale and negative zero.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecimalValue {
    kind: DecimalType,
    coefficient: i128,
    text: String,
}
impl DecimalValue {
    /// Validate fixed-point text before obtaining a native decimal coefficient.
    pub fn parse(kind: DecimalType, text: &str) -> Result<Self, DomainError> {
        let invalid = || DomainError::new(ErrorKind::InvalidDecimalValue).at("value");
        if !decimal_syntax(text) {
            return Err(invalid());
        }
        let unsigned = text.strip_prefix('-').unwrap_or(text);
        let unscaled = if kind.scale > 0 {
            let Some((whole, fraction)) = unsigned.split_once('.') else {
                return Err(invalid());
            };
            if fraction.len() != kind.scale as usize {
                return Err(invalid());
            }
            format!("{whole}{fraction}")
        } else {
            if unsigned.contains('.') {
                return Err(invalid());
            }
            if kind.scale < 0 && unsigned != "0" {
                let zeros = "0".repeat(usize::from(kind.scale.unsigned_abs()));
                unsigned
                    .strip_suffix(&zeros)
                    .ok_or_else(invalid)?
                    .to_owned()
            } else {
                unsigned.to_owned()
            }
        };
        let significant = unscaled.trim_start_matches('0');
        if significant.len().max(1) > usize::from(kind.precision) {
            return Err(invalid());
        }
        let magnitude = if significant.is_empty() {
            0
        } else {
            significant.parse::<i128>().map_err(|_| invalid())?
        };
        let coefficient = if text.starts_with('-') {
            -magnitude
        } else {
            magnitude
        };
        Ok(Self {
            kind,
            coefficient,
            text: text.to_owned(),
        })
    }
    /// Original text; trailing digits are retained exactly.
    pub fn as_str(&self) -> &str {
        &self.text
    }
    /// Declared decimal type.
    pub fn kind(&self) -> DecimalType {
        self.kind
    }
    /// Exact unscaled coefficient, always within decimal128 precision.
    pub fn coefficient(&self) -> i128 {
        self.coefficient
    }
}
/// Validated Gregorian calendar date in years 0001–9999.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DateValue(String);
impl DateValue {
    /// Validate fixed-width text, month lengths and Gregorian leap years.
    pub fn parse(text: &str) -> Result<Self, DomainError> {
        let invalid = || DomainError::new(ErrorKind::InvalidDateTime);
        if !text.is_ascii() || text.len() != 10 {
            return Err(invalid());
        }
        let mut parts = text.split('-');
        let year = parts
            .next()
            .filter(|s| s.len() == 4 && digits(s))
            .ok_or_else(invalid)?;
        let month = parts
            .next()
            .filter(|s| s.len() == 2 && digits(s))
            .ok_or_else(invalid)?;
        let day = parts
            .next()
            .filter(|s| s.len() == 2 && digits(s))
            .ok_or_else(invalid)?;
        if parts.next().is_some() {
            return Err(invalid());
        }
        let y = year.parse::<u32>().map_err(|_| invalid())?;
        let m = month.parse::<u32>().map_err(|_| invalid())?;
        let d = day.parse::<u32>().map_err(|_| invalid())?;
        let leap = y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
        let days = match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return Err(invalid()),
        };
        if y == 0 || d == 0 || d > days {
            return Err(invalid());
        }
        Ok(Self(text.to_owned()))
    }
    /// Exact ISO date carrier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
/// Declared timestamp resolution; there is no implicit unit reduction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeUnit {
    /// Whole seconds.
    Seconds,
    /// Three fractional digits.
    Milliseconds,
    /// Six fractional digits.
    Microseconds,
    /// Nine fractional digits.
    Nanoseconds,
}
impl TimeUnit {
    /// Number of fractional digits in the calendar carrier.
    pub fn fractional_digits(self) -> usize {
        match self {
            Self::Seconds => 0,
            Self::Milliseconds => 3,
            Self::Microseconds => 6,
            Self::Nanoseconds => 9,
        }
    }
}
/// Timezone carrier; existence in a zone database is an adapter capability check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Timezone(String);
impl Timezone {
    /// Accept UTC or slash-separated IANA-style text, without resolving local time.
    pub fn parse(text: &str) -> Result<Self, DomainError> {
        let mut parts = text.split('/');
        let first = parts.next().is_some_and(|s| {
            !s.is_empty()
                && s.bytes()
                    .all(|c| c.is_ascii_alphabetic() || b"_+-".contains(&c))
        });
        let rest: Vec<_> = parts.collect();
        let valid = first
            && !rest.is_empty()
            && rest.iter().all(|s| {
                !s.is_empty()
                    && s.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_+-".contains(&c))
            });
        if text != "UTC" && !valid {
            return Err(DomainError::new(ErrorKind::InvalidTimezone));
        }
        Ok(Self(text.to_owned()))
    }
    /// Preserved zone spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
/// Timestamp type distinguishes naive calendar time from a named-zone instant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimestampType {
    unit: TimeUnit,
    timezone: Option<Timezone>,
}
impl TimestampType {
    /// Keep unit and timezone explicit; None never implies UTC.
    pub fn new(unit: TimeUnit, timezone: Option<Timezone>) -> Self {
        Self { unit, timezone }
    }
    /// Declared resolution.
    pub fn unit(&self) -> TimeUnit {
        self.unit
    }
    /// Original zone; None means naive time.
    pub fn timezone(&self) -> Option<&Timezone> {
        self.timezone.as_ref()
    }
}
/// Calendar timestamp carrier, preserving nanoseconds without epoch conversion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimestampValue {
    kind: TimestampType,
    text: String,
}
impl TimestampValue {
    /// Aware values require UTC text ending in Z; naive values forbid that suffix.
    pub fn parse(kind: TimestampType, text: &str) -> Result<Self, DomainError> {
        let invalid = || DomainError::new(ErrorKind::InvalidTimestamp).at("value");
        let raw = if kind.timezone.is_some() {
            text.strip_suffix('Z').ok_or_else(invalid)?
        } else {
            text
        };
        let Some((date, clock)) = raw.split_once('T') else {
            return Err(invalid());
        };
        DateValue::parse(date).map_err(|e| e.at("value"))?;
        let (clock, fraction) = clock
            .split_once('.')
            .map_or((clock, None), |(c, f)| (c, Some(f)));
        let mut parts = clock.split(':');
        for maximum in [23, 59, 59] {
            let field = parts
                .next()
                .filter(|s| s.len() == 2 && digits(s))
                .ok_or_else(invalid)?;
            if field.parse::<u8>().map_err(|_| invalid())? > maximum {
                return Err(invalid());
            }
        }
        if parts.next().is_some() {
            return Err(invalid());
        }
        match (kind.unit.fractional_digits(), fraction) {
            (0, None) => {}
            (n, Some(f)) if n > 0 && f.len() == n && digits(f) => {}
            _ => return Err(invalid()),
        }
        Ok(Self {
            kind,
            text: text.to_owned(),
        })
    }
    /// Exact calendar text, with no timezone or unit normalization.
    pub fn as_str(&self) -> &str {
        &self.text
    }
    /// Declared resolution and original zone.
    pub fn kind(&self) -> &TimestampType {
        &self.kind
    }
}
/// A scalar value; null and IEEE NaN are distinct variants/representations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScalarValue {
    /// SQL null with no payload.
    Null,
    /// Boolean truth value.
    Bool(bool),
    /// Unicode text.
    String(String),
    /// Raw bytes, without an implicit text encoding.
    Binary(Vec<u8>),
    /// Width-checked integer.
    Integer(IntegerValue),
    /// IEEE value with a preserved carrier.
    Float(FloatValue),
    /// Exact decimal128 carrier.
    Decimal(DecimalValue),
    /// Calendar date.
    Date(DateValue),
    /// Unit/zone-preserving timestamp.
    Timestamp(TimestampValue),
}
/// Nested logical value; transport must separately bound collection sizes and depth.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Value {
    /// One scalar.
    Scalar(ScalarValue),
    /// Ordered nested values.
    List(Vec<Value>),
    /// Ordered uniquely named values.
    Struct(StructValue),
}
/// Struct values cannot be constructed with duplicate member names.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructValue(Vec<(FieldName, Value)>);
impl StructValue {
    /// Validate uniqueness without sorting or rewriting the original field order.
    pub fn new(fields: Vec<(FieldName, Value)>) -> Result<Self, DomainError> {
        let mut names = BTreeSet::new();
        for (i, (name, _)) in fields.iter().enumerate() {
            if !names.insert(name.as_str()) {
                return Err(DomainError::new(ErrorKind::DuplicateName)
                    .at("name")
                    .at(i.to_string())
                    .at("fields"));
            }
        }
        Ok(Self(fields))
    }
    /// Immutable ordered members.
    pub fn fields(&self) -> &[(FieldName, Value)] {
        &self.0
    }
}
