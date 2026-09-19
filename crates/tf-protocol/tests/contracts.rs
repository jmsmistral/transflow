//! Test-only interpretation of the authored schema subset, not a production decoder.
#![allow(clippy::unwrap_used, clippy::expect_used)] // Fail the test on invalid fixture/schema setup.

use serde_json::Value;
use std::collections::BTreeSet;

const CASES: &str = include_str!("../../../schemas/fixtures/conformance.json");
const VERSION_CASES: &str = include_str!("../../../schemas/fixtures/versions.json");
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
const PYTHON_KEYWORDS: &str = "False None True and as assert async await break class continue def del elif else except finally for from global if import in is lambda nonlocal not or pass raise return try while with yield";

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit())
}
fn unsigned(s: &str) -> bool {
    digits(s) && (s == "0" || !s.starts_with('0'))
}
fn decimal_syntax(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let parts: Vec<_> = s.split('.').collect();
    (parts.len() == 1 || parts.len() == 2)
        && unsigned(parts[0])
        && (parts.len() == 1 || digits(parts[1]))
}
fn float_syntax(s: &str) -> bool {
    let parts: Vec<_> = s.split(['e', 'E']).collect();
    (parts.len() == 1 || parts.len() == 2)
        && decimal_syntax(parts[0])
        && (parts.len() == 1 || digits(parts[1].strip_prefix(['+', '-']).unwrap_or(parts[1])))
}
fn date(s: &str) -> bool {
    if !s.is_ascii() || s.len() != 10 || &s[4..5] != "-" || &s[7..8] != "-" {
        return false;
    }
    if !digits(&s[..4]) || !digits(&s[5..7]) || !digits(&s[8..]) {
        return false;
    }
    let y: u32 = s[..4].parse().unwrap();
    let m: usize = s[5..7].parse().unwrap();
    let d: u32 = s[8..].parse().unwrap();
    if y == 0 || !(1..=12).contains(&m) {
        return false;
    }
    let leap = y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
    let max = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ][m - 1];
    d > 0 && d <= max
}
fn base64(s: &str) -> bool {
    if !s.is_ascii() || !s.len().is_multiple_of(4) {
        return false;
    }
    let padding = s.len() - s.trim_end_matches('=').len();
    if padding > 2 {
        return false;
    }
    let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let body = &s[..s.len() - padding];
    if !body.chars().all(|c| alphabet.contains(c)) {
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
fn check_format(name: &str, s: &str) -> bool {
    match name.strip_prefix("transflow-").unwrap_or(name) {
        "uuid" => {
            s.len() == 36
                && s.bytes().enumerate().all(|(i, c)| {
                    if [8, 13, 18, 23].contains(&i) {
                        c == b'-'
                    } else {
                        c.is_ascii_digit() || (b'a'..=b'f').contains(&c)
                    }
                })
        }
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
        kind @ ("i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64") => {
            let raw = s.strip_prefix('-').unwrap_or(s);
            if !unsigned(raw) || s == "-0" {
                return false;
            }
            let Ok(value) = s.parse::<i128>() else {
                return false;
            };
            let bits: u32 = kind[1..].parse().unwrap();
            let (lo, hi) = if kind.starts_with('i') {
                (-(1_i128 << (bits - 1)), (1_i128 << (bits - 1)) - 1)
            } else {
                (0, (1_i128 << bits) - 1)
            };
            (lo..=hi).contains(&value)
        }
        kind @ ("f32" | "f64") => {
            if ["NaN", "Infinity", "-Infinity"].contains(&s) {
                return true;
            }
            if !float_syntax(s) {
                return false;
            }
            let Ok(value) = s.parse::<f64>() else {
                return false;
            };
            let nonzero = s
                .split(['e', 'E'])
                .next()
                .unwrap()
                .bytes()
                .any(|c| matches!(c, b'1'..=b'9'));
            value.is_finite()
                && (value != 0.0 || !nonzero)
                && (kind == "f64"
                    || ((value as f32).is_finite() && (value as f32 != 0.0 || !nonzero)))
        }
        "date" => date(s),
        "base64" => base64(s),
        "relative-path" => {
            !s.is_empty()
                && s.chars().count() <= 1024
                && !s.contains(['\\', ':'])
                && s.chars().all(|c| c >= ' ' && c != '\u{7f}')
                && s.split('/').all(|p| !["", ".", ".."].contains(&p))
        }
        "name" => {
            !s.is_empty()
                && s.chars().count() <= 256
                && s.chars().all(|c| c >= ' ' && c != '\u{7f}')
        }
        "dataset-path" => s.split('/').all(|p| {
            p.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
                && p.bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
                && !PYTHON_KEYWORDS.split_whitespace().any(|word| word == p)
        }),
        "timezone" => {
            s == "UTC" || {
                let parts: Vec<_> = s.split('/').collect();
                parts.len() > 1
                    && parts.iter().enumerate().all(|(i, p)| {
                        !p.is_empty()
                            && p.bytes().all(|c| {
                                c.is_ascii_alphabetic()
                                    || b"_+-".contains(&c)
                                    || (i > 0 && c.is_ascii_digit())
                            })
                    })
            }
        }
        _ => false,
    }
}
fn invariant(name: &str, value: &Value) -> bool {
    match name {
        "decimal-value" => {
            let s = value["value"].as_str().unwrap();
            if !decimal_syntax(s) {
                return false;
            }
            let raw = s.strip_prefix('-').unwrap_or(s);
            let scale = value["scale"].as_f64().unwrap() as i64;
            let unscaled = if scale > 0 {
                let Some((whole, frac)) = raw.split_once('.') else {
                    return false;
                };
                if frac.len() != scale as usize {
                    return false;
                }
                format!("{whole}{frac}")
            } else {
                if raw.contains('.') {
                    return false;
                }
                if scale < 0 && raw != "0" {
                    let zeros = "0".repeat((-scale) as usize);
                    let Some(prefix) = raw.strip_suffix(&zeros) else {
                        return false;
                    };
                    prefix.to_string()
                } else {
                    raw.to_string()
                }
            };
            unscaled.trim_start_matches('0').len().max(1)
                <= value["precision"].as_f64().unwrap() as usize
        }
        "timestamp-value" => {
            let mut s = value["value"].as_str().unwrap();
            if !value["timezone"].is_null() {
                let Some(raw) = s.strip_suffix('Z') else {
                    return false;
                };
                s = raw;
            }
            if !s.is_ascii()
                || s.len() < 19
                || !date(&s[..10])
                || &s[10..11] != "T"
                || &s[13..14] != ":"
                || &s[16..17] != ":"
            {
                return false;
            }
            for (part, max) in [(&s[11..13], 23), (&s[14..16], 59), (&s[17..19], 59)] {
                if !digits(part) || part.parse::<u32>().unwrap() > max {
                    return false;
                }
            }
            let count = match value["unit"].as_str().unwrap() {
                "s" => 0,
                "ms" => 3,
                "us" => 6,
                "ns" => 9,
                _ => return false,
            };
            let tail = &s[19..];
            if count == 0 {
                tail.is_empty()
            } else {
                tail.starts_with('.') && tail.len() == count + 1 && digits(&tail[1..])
            }
        }
        "field-names" | "file-names" => {
            let (items, key) = if name == "field-names" {
                (&value["fields"], "name")
            } else {
                (&value["files"], "path")
            };
            let items = items.as_array().unwrap();
            items
                .iter()
                .map(|v| v[key].as_str().unwrap())
                .collect::<BTreeSet<_>>()
                .len()
                == items.len()
        }
        "catalog-entries" => {
            let entries = value["entries"].as_array().unwrap();
            let paths = entries
                .iter()
                .map(|e| e["path"].as_str().unwrap())
                .collect::<BTreeSet<_>>();
            let keys = entries
                .iter()
                .map(|e| {
                    (
                        e["key"]["workspace_id"].as_str().unwrap(),
                        e["key"]["dataset_id"].as_str().unwrap(),
                    )
                })
                .collect::<BTreeSet<_>>();
            paths.len() == entries.len()
                && keys.len() == entries.len()
                && entries.iter().all(|e| {
                    let foreign = e["key"]["workspace_id"] != value["workspace_id"];
                    (e["kind"] == "external") == foreign
                        && e["path"].as_str().unwrap().starts_with("external/") == foreign
                })
        }
        "frame-capabilities" => {
            let required = value["required_capabilities"].as_array().unwrap();
            let extensions = value["extensions"].as_array().unwrap();
            let names = required
                .iter()
                .map(|c| c.as_str().unwrap())
                .collect::<BTreeSet<_>>();
            let extra = extensions
                .iter()
                .map(|c| c["capability"].as_str().unwrap())
                .collect::<BTreeSet<_>>();
            names.len() == required.len()
                && extra.len() == extensions.len()
                && names
                    .iter()
                    .chain(extra.iter())
                    .all(|n| *n == "diagnostic.note.v1")
                && extensions.iter().all(|e| e["value"]["type"] == "string")
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
fn validate(schema: &Value, value: &Value, defs: &Value, depth: usize) -> bool {
    let Some(map) = schema.as_object() else {
        return false;
    };
    if depth > 64 || map.keys().any(|k| !KEYWORDS.contains(&k.as_str())) {
        return false;
    }
    if let Some(reference) = schema["$ref"].as_str() {
        let Some(key) = reference.strip_prefix("#/$defs/") else {
            return false;
        };
        return validate(&defs[key], value, defs, depth + 1);
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
            .is_some_and(|s| schema["format"].as_str().is_none_or(|f| check_format(f, s))),
        Some("integer") => value.as_f64().is_some_and(|n| {
            n.is_finite()
                && n.fract() == 0.0
                && n >= schema["minimum"].as_f64().unwrap()
                && n <= schema["maximum"].as_f64().unwrap()
        }),
        Some("boolean") => value.is_boolean(),
        Some("null") => value.is_null(),
        Some("array") => value.as_array().is_some_and(|items| {
            items.len() >= schema["minItems"].as_u64().unwrap() as usize
                && schema["maxItems"]
                    .as_u64()
                    .is_none_or(|max| items.len() <= max as usize)
                && items
                    .iter()
                    .all(|v| validate(&schema["items"], v, defs, depth + 1))
        }),
        Some("object") => value.as_object().is_some_and(|obj| {
            let required = schema["required"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            required
                .iter()
                .all(|k| obj.contains_key(k.as_str().unwrap()))
                && obj.iter().all(|(k, v)| {
                    let field = schema["properties"]
                        .get(k)
                        .unwrap_or(&schema["additionalProperties"]);
                    field.is_object() && validate(field, v, defs, depth + 1)
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

#[test]
fn shared_schema_cases_round_trip_without_loss() {
    let schema: Value = serde_json::from_str(tf_protocol::CONTRACT_SCHEMA).unwrap();
    let cases: Vec<Value> = serde_json::from_str(CASES).unwrap();
    for case in cases {
        let valid = validate(
            &schema["$defs"][case["schema"].as_str().unwrap()],
            &case["value"],
            &schema["$defs"],
            0,
        );
        assert_eq!(valid, case["valid"].as_bool().unwrap(), "{}", case["name"]);
        if valid {
            let encoded = serde_json::to_string(&case["value"]).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&encoded).unwrap(),
                case["value"]
            );
        }
    }
}
#[test]
fn independent_version_compatibility() {
    let versions: Value = serde_json::from_str(tf_protocol::FORMAT_VERSIONS).unwrap();
    let cases: Vec<Value> = serde_json::from_str(VERSION_CASES).unwrap();
    for case in cases {
        assert_eq!(
            versions.get(case["format"].as_str().unwrap()) == Some(&case["version"]),
            case["valid"].as_bool().unwrap()
        );
    }
}
#[test]
fn unknown_schema_rules_fail_closed() {
    assert!(!validate(
        &serde_json::json!({"newRequiredRule":true}),
        &Value::Null,
        &Value::Null,
        0
    ));
}
