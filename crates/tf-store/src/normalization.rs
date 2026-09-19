//! Lossless logical projection of qualified Arrow/Parquet types; no implicit casts.
use arrow_array::{temporal_conversions::*, *};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use serde_json::{Value, json};
use tf_protocol::canonical::schema_digest;

/// A field-local failure; callers render the path as untrusted text.
#[derive(Debug, thiserror::Error)]
#[error("Schema normalization failed at {path}: {reason}")]
pub struct NormalizationError {
    /// JSON-style field/index location without data values.
    pub path: String,
    /// Stable explanation; never contains cell contents.
    pub reason: &'static str,
}
fn fail(path: &str, reason: &'static str) -> NormalizationError {
    NormalizationError {
        path: path.into(),
        reason,
    }
}
/// Execution capability is separate from representability in the catalogue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Adapter {
    /// Canonical Arrow/Parquet storage path.
    Parquet,
    /// Initial Polars transform boundary.
    Polars,
    /// Exact canonical DuckDB validation boundary.
    DuckDbValidation,
    /// Deferred pandas transform adapter.
    Pandas,
    /// Deferred SQL transform adapter.
    DuckDbSql,
}
/// Validated normalized schema, with an independent canonical fingerprint.
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedSchema {
    value: Value,
    fingerprint: String,
}
impl NormalizedSchema {
    /// Project without changing nullability, signedness, precision or timezone.
    /// List child names normalize to `element`; their names are physical conventions.
    pub fn from_arrow(schema: &Schema) -> Result<Self, NormalizationError> {
        if !schema.metadata().is_empty() {
            return Err(fail(
                "$",
                "Schema-level engine metadata requires explicit removal",
            ));
        }
        let mut remaining = 4096usize;
        let mut metadata_bytes = 1024 * 1024usize;
        let fields = schema
            .fields()
            .iter()
            .map(|f| field(f, "$", 0, &mut remaining, &mut metadata_bytes, false))
            .collect::<Result<Vec<_>, _>>()?;
        let value = json!({"format_version":1,"fields":fields});
        let fingerprint = schema_digest(&value)
            .map_err(|_| {
                fail(
                    "$",
                    "Invalid or duplicate field names, metadata or logical types",
                )
            })?
            .hex();
        Ok(Self { value, fingerprint })
    }
    /// Closed LogicalSchemaV1 payload, independent of Arrow implementation classes.
    pub fn value(&self) -> &Value {
        &self.value
    }
    /// Purpose-separated schema digest.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
    /// Reject an unsupported engine boundary before an engine can silently cast.
    pub fn require(&self, adapter: Adapter) -> Result<(), NormalizationError> {
        if matches!(adapter, Adapter::Pandas | Adapter::DuckDbSql) {
            return Err(fail("$", "This transform adapter is not implemented"));
        }
        fn visit(value: &Value, adapter: Adapter, path: &str) -> Result<(), NormalizationError> {
            match value["type"].as_str() {
                Some("decimal") => {
                    let scale = value["scale"]
                        .as_i64()
                        .ok_or_else(|| fail(path, "Invalid decimal scale"))?;
                    let precision = value["precision"]
                        .as_i64()
                        .ok_or_else(|| fail(path, "Invalid decimal precision"))?;
                    if scale < 0 || scale > precision {
                        return Err(fail(
                            path,
                            "Adapter requires decimal scale between zero and precision; cast explicitly",
                        ));
                    }
                }
                Some("timestamp") => {
                    if value["unit"] == "s" {
                        return Err(fail(
                            path,
                            "Second-resolution timestamps require an explicit unit conversion before this adapter",
                        ));
                    }
                    if adapter == Adapter::DuckDbValidation
                        && !value["timezone"].is_null()
                        && (value["timezone"] != "UTC" || value["unit"] == "ns")
                    {
                        return Err(fail(
                            path,
                            "DuckDB cannot preserve this timezone/precision contract exactly",
                        ));
                    }
                }
                Some("list") => visit(
                    &value["element"]["logical_type"],
                    adapter,
                    &format!("{path}[]"),
                )?,
                Some("struct") => {
                    for f in value["fields"]
                        .as_array()
                        .ok_or_else(|| fail(path, "Invalid fields"))?
                    {
                        visit(
                            &f["logical_type"],
                            adapter,
                            &format!("{path}.{}", f["name"].as_str().unwrap_or("?")),
                        )?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        for f in self.value["fields"]
            .as_array()
            .ok_or_else(|| fail("$", "Invalid fields"))?
        {
            visit(
                &f["logical_type"],
                adapter,
                &format!("$.{}", f["name"].as_str().unwrap_or("?")),
            )?;
        }
        Ok(())
    }
}
fn field(
    f: &Field,
    path: &str,
    depth: usize,
    remaining: &mut usize,
    metadata_bytes: &mut usize,
    element: bool,
) -> Result<Value, NormalizationError> {
    if depth > 24 || *remaining == 0 {
        return Err(fail(path, "Schema exceeds depth or field-count limit"));
    }
    *remaining -= 1;
    for bytes in std::iter::once(f.name().len() + 128)
        .chain(f.metadata().iter().flat_map(|(k, v)| [k.len(), v.len()]))
    {
        *metadata_bytes = metadata_bytes
            .checked_sub(bytes)
            .ok_or_else(|| fail(path, "Schema metadata exceeds 1 MiB budget"))?;
    }
    if let DataType::Timestamp(_, Some(zone)) = f.data_type() {
        *metadata_bytes = metadata_bytes
            .checked_sub(zone.len())
            .ok_or_else(|| fail(path, "Schema metadata exceeds 1 MiB budget"))?;
    }
    let path = format!("{path}.{}", f.name());
    if f.metadata()
        .keys()
        .any(|k| k.starts_with("ARROW:extension:"))
    {
        return Err(fail(
            &path,
            "Extension/object types require an explicit conversion",
        ));
    }
    let ty = match f.data_type() {
        DataType::Boolean => json!({"type":"bool"}),
        DataType::Int8 => json!({"type":"i8"}),
        DataType::Int16 => json!({"type":"i16"}),
        DataType::Int32 => json!({"type":"i32"}),
        DataType::Int64 => json!({"type":"i64"}),
        DataType::UInt8 => json!({"type":"u8"}),
        DataType::UInt16 => json!({"type":"u16"}),
        DataType::UInt32 => json!({"type":"u32"}),
        DataType::UInt64 => json!({"type":"u64"}),
        DataType::Float32 => json!({"type":"f32"}),
        DataType::Float64 => json!({"type":"f64"}),
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => json!({"type":"string"}),
        DataType::Binary | DataType::LargeBinary | DataType::BinaryView => json!({"type":"binary"}),
        DataType::Date32 => json!({"type":"date"}),
        DataType::Decimal128(p, s) => json!({"type":"decimal","precision":p,"scale":s}),
        DataType::Timestamp(u, tz) => {
            json!({"type":"timestamp","unit":unit(u),"timezone":tz.as_deref()})
        }
        DataType::List(child) | DataType::LargeList(child) => {
            json!({"type":"list","element":field(child,&path,depth+1,remaining,metadata_bytes,true)?})
        }
        DataType::Struct(fields) => {
            json!({"type":"struct","fields":fields.iter().map(|f|field(f,&path,depth+1,remaining,metadata_bytes,false)).collect::<Result<Vec<_>,_>>()?})
        }
        _ => {
            return Err(fail(
                &path,
                "Unsupported Arrow type; convert explicitly without losing semantics",
            ));
        }
    };
    let mut result = json!({"name":if element {"element"}else{f.name()},"logical_type":ty,"nullable":f.is_nullable()});
    if !f.metadata().is_empty() {
        result["metadata"] = json!(
            f.metadata()
                .iter()
                .collect::<std::collections::BTreeMap<_, _>>()
        );
    }
    Ok(result)
}
fn unit(u: &TimeUnit) -> &'static str {
    match u {
        TimeUnit::Second => "s",
        TimeUnit::Millisecond => "ms",
        TimeUnit::Microsecond => "us",
        TimeUnit::Nanosecond => "ns",
    }
}
fn down<T: Array + 'static>(array: &dyn Array) -> Result<&T, NormalizationError> {
    array
        .as_any()
        .downcast_ref()
        .ok_or_else(|| fail("$", "Arrow array/type disagreement"))
}
fn decimal(n: i128, p: u8, s: i8) -> Result<String, NormalizationError> {
    let digits = n.unsigned_abs().to_string();
    if digits.len() > usize::from(p) {
        return Err(fail("$", "Decimal coefficient exceeds declared precision"));
    }
    let sign = if n < 0 { "-" } else { "" };
    if s <= 0 {
        return Ok(format!(
            "{sign}{digits}{}",
            if n == 0 {
                String::new()
            } else {
                "0".repeat(usize::from(s.unsigned_abs()))
            }
        ));
    }
    let scale = s as usize;
    let padded = format!("{:0>width$}", digits, width = scale + 1);
    let split = padded.len() - scale;
    Ok(format!("{sign}{}.{}", &padded[..split], &padded[split..]))
}
fn timestamp(n: i64, u: &TimeUnit, aware: bool) -> Result<String, NormalizationError> {
    let dt = match u {
        TimeUnit::Second => timestamp_s_to_datetime(n),
        TimeUnit::Millisecond => timestamp_ms_to_datetime(n),
        TimeUnit::Microsecond => timestamp_us_to_datetime(n),
        TimeUnit::Nanosecond => timestamp_ns_to_datetime(n),
    }
    .ok_or_else(|| fail("$", "Timestamp outside supported calendar range"))?;
    let base = dt.format("%Y-%m-%dT%H:%M:%S").to_string();
    let digits = match u {
        TimeUnit::Second => 0,
        TimeUnit::Millisecond => 3,
        TimeUnit::Microsecond => 6,
        TimeUnit::Nanosecond => 9,
    };
    let fraction = if digits == 0 {
        String::new()
    } else {
        let nanos = dt.and_utc().timestamp_subsec_nanos();
        format!(
            ".{:0width$}",
            nanos / 10u32.pow(9 - digits),
            width = digits as usize
        )
    };
    let text = format!("{base}{fraction}{}", if aware { "Z" } else { "" });
    if base.len() != 19 || base.starts_with("0000") {
        return Err(fail("$", "Timestamp outside years 0001–9999"));
    }
    Ok(text)
}
/// Serialize one bounded cell, never an entire dataframe. Null and NaN stay distinct.
/// Naive timestamps retain wall-time ticks; aware timestamps use UTC text plus original zone.
pub fn cell(array: &dyn Array, index: usize) -> Result<Value, NormalizationError> {
    if index >= array.len() {
        return Err(fail("$", "Cell index out of bounds"));
    }
    let schema = Schema::new(vec![Field::new("value", array.data_type().clone(), true)]);
    NormalizedSchema::from_arrow(&schema)?;
    let value = encode(
        array,
        index,
        &mut Budget {
            nodes: 4096,
            bytes: 64 * 1024,
        },
        0,
    )?;
    validate_value(
        array,
        &Field::new("value", array.data_type().clone(), true),
        index,
        "$",
        0,
    )?;
    if tf_protocol::canonical::canonical_json(&value)
        .map_err(|_| fail("$", "Invalid cell encoding"))?
        .len()
        > 64 * 1024
    {
        return Err(fail("$", "Encoded cell exceeds 64 KiB limit"));
    }
    tf_protocol::validate_document("WireValue", &value)
        .map_err(|_| fail("$", "Value outside portable wire range"))?;
    Ok(value)
}
struct Budget {
    nodes: usize,
    bytes: usize,
}
impl Budget {
    fn use_bytes(&mut self, n: usize) -> Result<(), NormalizationError> {
        self.bytes = self
            .bytes
            .checked_sub(n)
            .ok_or_else(|| fail("$", "Cell exceeds 64 KiB payload limit"))?;
        Ok(())
    }
}
fn encode(a: &dyn Array, i: usize, b: &mut Budget, d: usize) -> Result<Value, NormalizationError> {
    if d > 24 || b.nodes == 0 {
        return Err(fail("$", "Cell exceeds nesting or element limit"));
    }
    b.nodes -= 1;
    b.use_bytes(32)?;
    if a.is_null(i) {
        return Ok(json!({"type":"null"}));
    }
    macro_rules! integer {($t:ty,$name:expr)=>{json!({"type":$name,"value":down::<$t>(a)?.value(i).to_string()})};}
    macro_rules! string {($t:ty)=>{{let s=down::<$t>(a)?.value(i);b.use_bytes(s.len())?;json!({"type":"string","value":s})}};}
    macro_rules! binary {($t:ty)=>{{let bytes=down::<$t>(a)?.value(i);b.use_bytes(bytes.len().saturating_add(2)/3*4)?;json!({"type":"binary","value":base64(bytes)})}};}
    macro_rules! time {($t:ty,$u:expr,$tz:expr)=>{json!({"type":"timestamp","unit":unit($u),"timezone":$tz.as_deref(),"value":timestamp(down::<$t>(a)?.value(i),$u,$tz.is_some())?})};}
    Ok(match a.data_type() {
        DataType::Boolean => json!({"type":"bool","value":down::<BooleanArray>(a)?.value(i)}),
        DataType::Int8 => integer!(Int8Array, "i8"),
        DataType::Int16 => integer!(Int16Array, "i16"),
        DataType::Int32 => integer!(Int32Array, "i32"),
        DataType::Int64 => integer!(Int64Array, "i64"),
        DataType::UInt8 => integer!(UInt8Array, "u8"),
        DataType::UInt16 => integer!(UInt16Array, "u16"),
        DataType::UInt32 => integer!(UInt32Array, "u32"),
        DataType::UInt64 => integer!(UInt64Array, "u64"),
        DataType::Float32 => {
            json!({"type":"f32","value":float(down::<Float32Array>(a)?.value(i) as f64)})
        }
        DataType::Float64 => json!({"type":"f64","value":float(down::<Float64Array>(a)?.value(i))}),
        DataType::Utf8 => string!(StringArray),
        DataType::LargeUtf8 => string!(LargeStringArray),
        DataType::Utf8View => string!(StringViewArray),
        DataType::Binary => binary!(BinaryArray),
        DataType::LargeBinary => binary!(LargeBinaryArray),
        DataType::BinaryView => binary!(BinaryViewArray),
        DataType::Decimal128(p, s) => {
            json!({"type":"decimal","precision":p,"scale":s,"value":decimal(down::<Decimal128Array>(a)?.value(i),*p,*s)?})
        }
        DataType::Date32 => {
            json!({"type":"date","value":date32_to_datetime(down::<Date32Array>(a)?.value(i)).ok_or_else(||fail("$","Date outside portable calendar range"))?.format("%Y-%m-%d").to_string()})
        }
        DataType::Timestamp(u, tz) => match u {
            TimeUnit::Second => time!(TimestampSecondArray, u, tz),
            TimeUnit::Millisecond => time!(TimestampMillisecondArray, u, tz),
            TimeUnit::Microsecond => time!(TimestampMicrosecondArray, u, tz),
            TimeUnit::Nanosecond => time!(TimestampNanosecondArray, u, tz),
        },
        DataType::List(_) => {
            let values = down::<ListArray>(a)?.value(i);
            list(values.as_ref(), b, d)?
        }
        DataType::LargeList(_) => {
            let values = down::<LargeListArray>(a)?.value(i);
            list(values.as_ref(), b, d)?
        }
        DataType::Struct(fields) => {
            let a = down::<StructArray>(a)?;
            let mut values = Vec::new();
            for (f, column) in fields.iter().zip(a.columns()) {
                b.use_bytes(f.name().len())?;
                values.push(json!({"name":f.name(),"value":encode(column.as_ref(),i,b,d+1)?}));
            }
            json!({"type":"struct","fields":values})
        }
        _ => return Err(fail("$", "Unsupported array type")),
    })
}
fn list(a: &dyn Array, b: &mut Budget, d: usize) -> Result<Value, NormalizationError> {
    if a.len() > b.nodes {
        return Err(fail("$", "Cell exceeds element limit"));
    }
    let values = (0..a.len())
        .map(|i| encode(a, i, b, d + 1))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({"type":"list","values":values}))
}
fn float(n: f64) -> String {
    if n.is_nan() {
        "NaN".into()
    } else if n == f64::INFINITY {
        "Infinity".into()
    } else if n == f64::NEG_INFINITY {
        "-Infinity".into()
    } else {
        n.to_string()
    }
}
fn base64(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let a = c[0];
        let b = c.get(1).copied().unwrap_or(0);
        let d = c.get(2).copied().unwrap_or(0);
        out.push(DIGITS[(a >> 2) as usize] as char);
        out.push(DIGITS[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        out.push(if c.len() > 1 {
            DIGITS[(((b & 15) << 2) | (d >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            DIGITS[(d & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Validate actual values without converting a table to JSON or applying casts.
/// Null children masked by a null parent are not logically present.
pub fn validate_batch(
    batch: &RecordBatch,
    expected: &NormalizedSchema,
) -> Result<(), NormalizationError> {
    let actual = NormalizedSchema::from_arrow(batch.schema().as_ref())?;
    if &actual != expected {
        return Err(fail(
            "$",
            "Batch schema differs from the accepted logical schema",
        ));
    }
    for (field, array) in batch.schema().fields().iter().zip(batch.columns()) {
        let path = format!("$.{}", field.name());
        for row in 0..batch.num_rows() {
            validate_value(array.as_ref(), field, row, &path, 0)?;
        }
    }
    Ok(())
}
fn validate_value(
    a: &dyn Array,
    f: &Field,
    i: usize,
    path: &str,
    depth: usize,
) -> Result<(), NormalizationError> {
    if depth > 24 {
        return Err(fail(path, "Value nesting exceeds limit"));
    }
    if a.is_null(i) {
        return if f.is_nullable() {
            Ok(())
        } else {
            Err(fail(path, "Null in a non-nullable logical field"))
        };
    }
    match a.data_type() {
        DataType::Date32 => {
            let n = down::<Date32Array>(a)?.value(i);
            if !(-719162..=2932896).contains(&n) {
                return Err(fail(path, "Date outside years 0001–9999"));
            }
        }
        DataType::Decimal128(p, s) => {
            decimal(down::<Decimal128Array>(a)?.value(i), *p, *s)
                .map_err(|e| fail(path, e.reason))?;
        }
        DataType::Timestamp(u, tz) => {
            let n = match u {
                TimeUnit::Second => down::<TimestampSecondArray>(a)?.value(i),
                TimeUnit::Millisecond => down::<TimestampMillisecondArray>(a)?.value(i),
                TimeUnit::Microsecond => down::<TimestampMicrosecondArray>(a)?.value(i),
                TimeUnit::Nanosecond => down::<TimestampNanosecondArray>(a)?.value(i),
            };
            timestamp(n, u, tz.is_some()).map_err(|e| fail(path, e.reason))?;
        }
        DataType::List(child) | DataType::LargeList(child) => {
            let values = if matches!(a.data_type(), DataType::List(_)) {
                down::<ListArray>(a)?.value(i)
            } else {
                down::<LargeListArray>(a)?.value(i)
            };
            for j in 0..values.len() {
                validate_value(values.as_ref(), child, j, &format!("{path}[]"), depth + 1)?;
            }
        }
        DataType::Struct(fields) => {
            for (field, array) in fields.iter().zip(down::<StructArray>(a)?.columns()) {
                validate_value(
                    array.as_ref(),
                    field,
                    i,
                    &format!("{path}.{}", field.name()),
                    depth + 1,
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}
/// Schema and exact decoded row count, ready for artifact manifest construction.
#[derive(Debug, Clone)]
pub struct NormalizedFile {
    /// Portable logical schema; physical string/list offset widths are erased.
    pub schema: NormalizedSchema,
    /// All decoded rows, including zero.
    pub rows: u64,
}
/// Decode a complete Parquet file through a caller-owned stable descriptor.
/// Callers guard identity/hashes and ownership; this function does not publish data.
pub fn inspect_parquet(mut file: std::fs::File) -> Result<NormalizedFile, NormalizationError> {
    use std::io::{Read, Seek, SeekFrom};
    let io = |_| fail("$", "Parquet bytes could not be read");
    let len = file.metadata().map_err(io)?.len();
    if len < 12 {
        return Err(fail("$", "Invalid Parquet footer"));
    }
    file.seek(SeekFrom::End(-8)).map_err(io)?;
    let mut footer = [0u8; 8];
    file.read_exact(&mut footer).map_err(io)?;
    let size = u32::from_le_bytes(
        footer[..4]
            .try_into()
            .map_err(|_| fail("$", "Invalid footer"))?,
    ) as u64;
    if &footer[4..] != b"PAR1" || size > 16 * 1024 * 1024 || size > len - 12 {
        return Err(fail("$", "Invalid or oversized Parquet footer"));
    }
    file.rewind().map_err(io)?;
    let builder = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|_| fail("$", "Parquet metadata is unreadable"))?;
    let schema = NormalizedSchema::from_arrow(builder.schema().as_ref())?;
    schema.require(Adapter::Parquet)?;
    let expected = u64::try_from(builder.metadata().file_metadata().num_rows())
        .map_err(|_| fail("$", "Invalid Parquet row count"))?;
    let mut rows = 0u64;
    for batch in builder
        .with_batch_size(8192)
        .build()
        .map_err(|_| fail("$", "Parquet reader could not be created"))?
    {
        let batch = batch.map_err(|_| fail("$", "Parquet rows could not be decoded"))?;
        validate_batch(&batch, &schema)?;
        rows = rows
            .checked_add(batch.num_rows() as u64)
            .ok_or_else(|| fail("$", "Row count overflow"))?;
    }
    if rows != expected {
        return Err(fail("$", "Decoded row count differs from Parquet footer"));
    }
    Ok(NormalizedFile { schema, rows })
}
