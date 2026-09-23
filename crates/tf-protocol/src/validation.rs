//! Runtime assertions for the authored JSON Schema subset and Transflow formats.

use crate::{CONTRACT_SCHEMA, ProtocolError};
use serde_json::Value;
use std::{collections::BTreeSet, sync::OnceLock};
use tf_domain::{DatasetId, DatasetPath, schema::FieldName, value::*};

const KEYWORDS: &[&str] = &[
    "$ref",
    "type",
    "const",
    "enum",
    "oneOf",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "minItems",
    "maxItems",
    "minimum",
    "maximum",
    "format",
    "x-transflow-invariant",
];
static SCHEMA: OnceLock<Result<Value, serde_json::Error>> = OnceLock::new();

/// Validate a named definition, including semantic assertions. Unknown definitions fail.
pub fn validate_document(name: &str, value: &Value) -> Result<(), ProtocolError> {
    let schema = SCHEMA
        .get_or_init(|| serde_json::from_str(CONTRACT_SCHEMA))
        .as_ref()
        .map_err(|_| ProtocolError::Schema)?;
    let defs = &schema["$defs"];
    if validate(&defs[name], value, defs, 0) {
        Ok(())
    } else {
        Err(ProtocolError::InvalidDocument)
    }
}
fn integer_kind(name: &str) -> Option<IntegerType> {
    Some(match name {
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
fn base64(s: &str) -> bool {
    if !s.is_ascii() || !s.len().is_multiple_of(4) {
        return false;
    }
    let body = s.trim_end_matches('=');
    let padding = s.len() - body.len();
    let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    if padding > 2 || !body.chars().all(|c| alphabet.contains(c)) {
        return false;
    }
    if padding == 0 {
        return true;
    }
    let Some(last) = body.chars().last().and_then(|c| alphabet.find(c)) else {
        return false;
    };
    if padding == 2 {
        last % 16 == 0
    } else {
        last % 4 == 0
    }
}
fn format(name: &str, s: &str) -> bool {
    let name = name.strip_prefix("transflow-").unwrap_or(name);
    if let Some(kind) = integer_kind(name) {
        return IntegerValue::parse(kind, s).is_ok();
    }
    match name {
        "uuid" => s.parse::<DatasetId>().is_ok(),
        "diagnostic-text" => {
            !s.is_empty()
                && s.len() <= 32768
                && !s.chars().any(tf_domain::diagnostic::unsafe_character)
        }
        "sha256" => {
            s.len() == 64
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        }
        "f32" => FloatValue::parse(FloatWidth::F32, s).is_ok(),
        "f64" => FloatValue::parse(FloatWidth::F64, s).is_ok(),
        "date" => DateValue::parse(s).is_ok(),
        "base64" => base64(s),
        "name" => FieldName::new(s).is_ok(),
        "dataset-path" => s.parse::<DatasetPath>().is_ok(),
        "timezone" => Timezone::parse(s).is_ok(),
        "relative-path" => {
            !s.is_empty()
                && s.chars().count() <= 1024
                && !s.contains(['\\', ':'])
                && !s.chars().any(|c| c.is_ascii_control())
                && s.split('/').all(|p| !["", ".", ".."].contains(&p))
        }
        _ => false,
    }
}
fn same(a: &Value, b: &Value) -> bool {
    if a.is_number() && b.is_number() {
        a.as_f64() == b.as_f64()
    } else {
        a == b
    }
}
fn small_integer(v: &Value) -> Option<i64> {
    v.as_f64()
        .filter(|n| n.is_finite() && n.fract() == 0.0 && n.abs() <= u32::MAX as f64)
        .map(|n| n as i64)
}
fn decimal(v: &Value) -> bool {
    let Some((p, s)) = small_integer(&v["precision"]).zip(small_integer(&v["scale"])) else {
        return false;
    };
    let (Ok(p), Ok(s), Some(text)) = (u8::try_from(p), i8::try_from(s), v["value"].as_str()) else {
        return false;
    };
    DecimalType::new(p, s)
        .and_then(|kind| DecimalValue::parse(kind, text))
        .is_ok()
}
fn timestamp(v: &Value) -> bool {
    let unit = match v["unit"].as_str() {
        Some("s") => TimeUnit::Seconds,
        Some("ms") => TimeUnit::Milliseconds,
        Some("us") => TimeUnit::Microseconds,
        Some("ns") => TimeUnit::Nanoseconds,
        _ => return false,
    };
    let timezone = if v["timezone"].is_null() {
        None
    } else {
        let Some(text) = v["timezone"].as_str() else {
            return false;
        };
        let Ok(zone) = Timezone::parse(text) else {
            return false;
        };
        Some(zone)
    };
    v["value"]
        .as_str()
        .is_some_and(|text| TimestampValue::parse(TimestampType::new(unit, timezone), text).is_ok())
}
fn unique(items: &[Value], key: &str) -> bool {
    let names: Option<BTreeSet<_>> = items.iter().map(|v| v[key].as_str()).collect();
    names.is_some_and(|names| names.len() == items.len())
}
fn invariant(name: &str, v: &Value) -> bool {
    match name {
        "decimal-value" => decimal(v),
        "timestamp-value" => timestamp(v),
        "field-names" => v["fields"].as_array().is_some_and(|a| unique(a, "name")),
        "file-names" => v["files"].as_array().is_some_and(|a| unique(a, "path")),
        "catalog-entries" => {
            let (Some(entries), Some(owner)) =
                (v["entries"].as_array(), v["workspace_id"].as_str())
            else {
                return false;
            };
            let mut keys = BTreeSet::new();
            let mut paths = BTreeSet::new();
            if !entries.iter().all(|e| {
                let (Some(workspace), Some(dataset), Some(path)) = (
                    e["key"]["workspace_id"].as_str(),
                    e["key"]["dataset_id"].as_str(),
                    e["path"].as_str(),
                ) else {
                    return false;
                };
                let foreign = workspace != owner;
                path != "external"
                    && paths.insert(path)
                    && keys.insert((workspace, dataset))
                    && (e["kind"] == "external") == foreign
                    && path.starts_with("external/") == foreign
            }) {
                return false;
            }
            match v.get("aliases") {
                None => true,
                Some(aliases) => aliases.as_array().is_some_and(|aliases| {
                    aliases.iter().all(|a| {
                        let (Some(workspace), Some(dataset), Some(path)) = (
                            a["key"]["workspace_id"].as_str(),
                            a["key"]["dataset_id"].as_str(),
                            a["path"].as_str(),
                        ) else {
                            return false;
                        };
                        path != "external"
                            && paths.insert(path)
                            && keys.contains(&(workspace, dataset))
                            && path.starts_with("external/") == (workspace != owner)
                    })
                }),
            }
        }
        "frame-capabilities" => {
            let (Some(required), Some(extensions)) = (
                v["required_capabilities"].as_array(),
                v["extensions"].as_array(),
            ) else {
                return false;
            };
            let names: Option<BTreeSet<_>> = required.iter().map(Value::as_str).collect();
            names.is_some_and(|names| {
                names.len() == required.len()
                    && names.iter().all(|s| {
                        matches!(
                            *s,
                            "diagnostic.note.v1"
                                | "discovery.v1"
                                | "polars.execute.v1"
                                | "expectation.ast.v1"
                                | "expectation.core.v1"
                                | "duckdb.checks.v1"
                        )
                    })
            }) && unique(extensions, "capability")
                && extensions.iter().all(|e| {
                    e["capability"] == "diagnostic.note.v1" && e["value"]["type"] == "string"
                })
        }
        _ => false,
    }
}
fn validate(schema: &Value, value: &Value, defs: &Value, depth: usize) -> bool {
    let Some(map) = schema.as_object() else {
        return false;
    };
    if depth > 64 || map.keys().any(|k| !KEYWORDS.contains(&k.as_str())) {
        return false;
    }
    // Reject a mismatched discriminator before descending into recursive children.
    // Object key order must not turn recursive oneOf schemas into exponential work.
    if let (Some(properties), Some(object)) = (schema["properties"].as_object(), value.as_object())
        && properties.iter().any(|(key, rule)| {
            rule.get("const")
                .is_some_and(|expected| object.get(key).is_some_and(|v| !same(v, expected)))
        })
    {
        return false;
    }
    if let Some(reference) = schema["$ref"].as_str() {
        return reference
            .strip_prefix("#/$defs/")
            .is_some_and(|key| validate(&defs[key], value, defs, depth + 1));
    }
    if let Some(branches) = schema["oneOf"].as_array()
        && branches
            .iter()
            .filter(|s| validate(s, value, defs, depth + 1))
            .count()
            != 1
    {
        return false;
    }
    if map.contains_key("const") && !same(value, &schema["const"]) {
        return false;
    }
    if let Some(choices) = schema["enum"].as_array()
        && !choices.iter().any(|v| same(value, v))
    {
        return false;
    }
    let shape = match schema["type"].as_str() {
        Some("string") => value
            .as_str()
            .is_some_and(|s| schema["format"].as_str().is_none_or(|f| format(f, s))),
        Some("integer") => value.as_f64().is_some_and(|n| {
            n.is_finite()
                && n.fract() == 0.0
                && schema["minimum"].as_f64().is_some_and(|min| n >= min)
                && schema["maximum"].as_f64().is_some_and(|max| n <= max)
        }),
        Some("boolean") => value.is_boolean(),
        Some("null") => value.is_null(),
        Some("array") => value.as_array().is_some_and(|items| {
            schema["minItems"]
                .as_u64()
                .is_none_or(|min| items.len() >= min as usize)
                && schema["maxItems"]
                    .as_u64()
                    .is_none_or(|max| items.len() <= max as usize)
                && items
                    .iter()
                    .all(|v| validate(&schema["items"], v, defs, depth + 1))
        }),
        Some("object") => value.as_object().is_some_and(|object| {
            let required = schema["required"].as_array().is_none_or(|keys| {
                keys.iter()
                    .all(|key| key.as_str().is_some_and(|k| object.contains_key(k)))
            });
            required
                && object.iter().all(|(key, v)| {
                    if let Some(member) = schema["properties"].get(key) {
                        validate(member, v, defs, depth + 1)
                    } else {
                        schema["additionalProperties"].is_object()
                            && validate(&schema["additionalProperties"], v, defs, depth + 1)
                    }
                })
        }),
        None => true,
        _ => false,
    };
    shape
        && schema["x-transflow-invariant"]
            .as_str()
            .is_none_or(|n| invariant(n, value))
}
