//! T013 fixture projection through real domain constructors, not a production JSON codec.
//! T012 separately checks envelopes/unknown members; T014 owns production decoding.
#![allow(clippy::unwrap_used, clippy::expect_used)] // Fixture parsing/assertions only.

use serde_json::{Value as Json, json};
use std::{collections::BTreeMap, error::Error};
use tf_domain::{DatasetId, DatasetKey, DatasetPath, DatasetScope, schema::*, value::*};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
const CASES: &str = include_str!("../../../schemas/fixtures/conformance.json");

fn text(v: &Json) -> Result<&str> {
    v.as_str().ok_or_else(|| "Expected string carrier".into())
}
fn integer_kind(tag: &str) -> Option<IntegerType> {
    Some(match tag {
        "i8" => IntegerType::I8,
        "i16" => IntegerType::I16,
        "i32" => IntegerType::I32,
        "i64" => IntegerType::I64,
        "u8" => IntegerType::U8,
        "u16" => IntegerType::U16,
        "u32" => IntegerType::U32,
        "u64" => IntegerType::U64,
        _ => return None,
    })
}
fn decimal_type(v: &Json) -> Result<DecimalType> {
    Ok(DecimalType::new(
        u8::try_from(v["precision"].as_u64().ok_or("precision")?)?,
        i8::try_from(v["scale"].as_i64().ok_or("scale")?)?,
    )?)
}
fn time_type(v: &Json) -> Result<TimestampType> {
    let unit = match text(&v["unit"])? {
        "s" => TimeUnit::Seconds,
        "ms" => TimeUnit::Milliseconds,
        "us" => TimeUnit::Microseconds,
        "ns" => TimeUnit::Nanoseconds,
        _ => return Err("unit".into()),
    };
    let timezone = if v["timezone"].is_null() {
        None
    } else {
        Some(Timezone::parse(text(&v["timezone"])?)?)
    };
    Ok(TimestampType::new(unit, timezone))
}
fn unit_text(unit: TimeUnit) -> &'static str {
    match unit {
        TimeUnit::Seconds => "s",
        TimeUnit::Milliseconds => "ms",
        TimeUnit::Microseconds => "us",
        TimeUnit::Nanoseconds => "ns",
    }
}
fn scalar_round_trip(v: &Json) -> Result<Json> {
    let tag = text(&v["type"])?;
    if let Some(kind) = integer_kind(tag) {
        let value = IntegerValue::parse(kind, text(&v["value"])?)?;
        return Ok(json!({"type":tag,"value":value.to_text()}));
    }
    Ok(match tag {
        "f32" | "f64" => {
            let width = if tag == "f32" {
                FloatWidth::F32
            } else {
                FloatWidth::F64
            };
            let value = FloatValue::parse(width, text(&v["value"])?)?;
            json!({"type":tag,"value":value.as_str()})
        }
        "decimal" => {
            let value = DecimalValue::parse(decimal_type(v)?, text(&v["value"])?)?;
            json!({"type":tag,"value":value.as_str(),"precision":value.kind().precision(),"scale":value.kind().scale()})
        }
        "date" => json!({"type":tag,"value":DateValue::parse(text(&v["value"])?)?.as_str()}),
        "timestamp" => {
            let value = TimestampValue::parse(time_type(v)?, text(&v["value"])?)?;
            json!({"type":tag,"value":value.as_str(),"unit":unit_text(value.kind().unit()),"timezone":value.kind().timezone().map(Timezone::as_str)})
        }
        _ => return Err("Outside scalar content projection".into()),
    })
}
fn logical_type(v: &Json) -> Result<LogicalType> {
    let tag = text(&v["type"])?;
    if let Some(kind) = integer_kind(tag) {
        return Ok(LogicalType::Integer(kind));
    }
    Ok(match tag {
        "bool" => LogicalType::Bool,
        "string" => LogicalType::String,
        "binary" => LogicalType::Binary,
        "date" => LogicalType::Date,
        "f32" => LogicalType::Float(FloatWidth::F32),
        "f64" => LogicalType::Float(FloatWidth::F64),
        "decimal" => LogicalType::Decimal(decimal_type(v)?),
        "timestamp" => LogicalType::Timestamp(time_type(v)?),
        "list" => LogicalType::List(Box::new(field(&v["element"])?)),
        "struct" => LogicalType::Struct(Fields::new(fields(&v["fields"])?)?),
        _ => return Err("unknown logical type".into()),
    })
}
fn fields(v: &Json) -> Result<Vec<Field>> {
    v.as_array().ok_or("fields")?.iter().map(field).collect()
}
fn field(v: &Json) -> Result<Field> {
    let metadata = v
        .get("metadata")
        .map(|m| -> Result<BTreeMap<String, String>> {
            m.as_object()
                .ok_or("metadata")?
                .iter()
                .map(|(k, v)| Ok((k.clone(), text(v)?.to_owned())))
                .collect()
        })
        .transpose()?;
    Ok(Field::new(
        FieldName::new(text(&v["name"])?)?,
        logical_type(&v["logical_type"])?,
        v["nullable"].as_bool().ok_or("nullable")?,
        metadata,
    ))
}
fn integer_tag(kind: IntegerType) -> &'static str {
    match kind {
        IntegerType::I8 => "i8",
        IntegerType::I16 => "i16",
        IntegerType::I32 => "i32",
        IntegerType::I64 => "i64",
        IntegerType::U8 => "u8",
        IntegerType::U16 => "u16",
        IntegerType::U32 => "u32",
        IntegerType::U64 => "u64",
    }
}
fn encode_type(kind: &LogicalType) -> Json {
    match kind {
        LogicalType::Bool => json!({"type":"bool"}),
        LogicalType::String => json!({"type":"string"}),
        LogicalType::Binary => json!({"type":"binary"}),
        LogicalType::Date => json!({"type":"date"}),
        LogicalType::Integer(kind) => json!({"type":integer_tag(*kind)}),
        LogicalType::Float(width) => json!({"type":if *width==FloatWidth::F32{"f32"}else{"f64"}}),
        LogicalType::Decimal(kind) => {
            json!({"type":"decimal","precision":kind.precision(),"scale":kind.scale()})
        }
        LogicalType::Timestamp(kind) => {
            json!({"type":"timestamp","unit":unit_text(kind.unit()),"timezone":kind.timezone().map(Timezone::as_str)})
        }
        LogicalType::List(element) => json!({"type":"list","element":encode_field(element)}),
        LogicalType::Struct(fields) => {
            json!({"type":"struct","fields":fields.as_slice().iter().map(encode_field).collect::<Vec<_>>()})
        }
    }
}
fn encode_field(field: &Field) -> Json {
    let mut value = json!({"name":field.name().as_str(),"logical_type":encode_type(field.logical_type()),"nullable":field.nullable()});
    if let Some(metadata) = field.metadata() {
        value["metadata"] = json!(metadata);
    }
    value
}
#[test]
fn shared_scalar_content_passes_through_production_constructors() {
    let cases: Vec<Json> = serde_json::from_str(CASES).unwrap();
    let mut checked = 0;
    for case in cases {
        if case["schema"] != "WireValue" || case["name"] == "unknown-required-field" {
            continue;
        }
        let tag = case["value"]["type"].as_str().unwrap();
        if integer_kind(tag).is_none()
            && !["f32", "f64", "decimal", "date", "timestamp"].contains(&tag)
        {
            continue;
        }
        checked += 1;
        let result = scalar_round_trip(&case["value"]);
        assert_eq!(
            result.is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}: {result:?}",
            case["name"]
        );
        if let Ok(value) = result {
            assert_eq!(value, case["value"], "{}", case["name"]);
        }
    }
    assert_eq!(checked, 101); // Explicit coverage, so fixture additions require review.
}
#[test]
fn shared_logical_schema_and_field_content_preserve_nested_types() {
    let cases: Vec<Json> = serde_json::from_str(CASES).unwrap();
    let mut checked = 0;
    for case in cases {
        let v = &case["value"];
        let result:Result<Json>=match case["schema"].as_str().unwrap() {
            "LogicalSchemaV1" if v["format_version"]==1=>{
                fields(&v["fields"]).and_then(|fields|Ok(LogicalSchema::new(fields)?)).map(|schema|json!({"format_version":1,"fields":schema.fields().iter().map(encode_field).collect::<Vec<_>>()}))
            },
            "Field"=>field(v).map(|f|encode_field(&f)),
            "LogicalType"=>logical_type(v).map(|kind|encode_type(&kind)),
            _=>continue,
        };
        checked += 1;
        assert_eq!(
            result.is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}: {result:?}",
            case["name"]
        );
        if let Ok(value) = result {
            assert_eq!(value, *v, "{}", case["name"]);
        }
    }
    assert_eq!(checked, 5);
}
#[test]
fn shared_catalogue_entries_preserve_owner_and_path() {
    let cases: Vec<Json> = serde_json::from_str(CASES).unwrap();
    let snapshot = cases
        .iter()
        .find(|c| c["schema"] == "CatalogSnapshotV1" && c["valid"] == true)
        .unwrap();
    let current = text(&snapshot["value"]["workspace_id"])
        .unwrap()
        .parse()
        .unwrap();
    let entries = snapshot["value"]["entries"].as_array().unwrap();
    for entry in entries {
        let key = DatasetKey::from_text(
            text(&entry["key"]["workspace_id"]).unwrap(),
            text(&entry["key"]["dataset_id"]).unwrap(),
        )
        .unwrap();
        let path = DatasetPath::parse_for_scope(text(&entry["path"]).unwrap(), key.scope(current))
            .unwrap();
        assert_eq!(
            json!({"workspace_id":key.workspace_id().to_string(),"dataset_id":key.dataset_id().to_string()}),
            entry["key"]
        );
        assert_eq!(path.as_str(), text(&entry["path"]).unwrap());
        assert_eq!(
            key.scope(current) == DatasetScope::Foreign,
            entry["kind"] == "external"
        );
    }
    for case in cases.iter().filter(|c| c["schema"] == "Uuid") {
        assert_eq!(
            text(&case["value"]).unwrap().parse::<DatasetId>().is_ok(),
            case["valid"].as_bool().unwrap()
        );
    }
}
