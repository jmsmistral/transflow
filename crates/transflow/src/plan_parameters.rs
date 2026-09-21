//! Normalize CLI JSON parameter values inside the validated planning scope.
use crate::build_plan::{Error, failure};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tf_catalog::{RegistrySnapshot, candidate::CandidateIdentity, validation::ValidatedGraph};

pub(crate) fn overrides(
    graph: &ValidatedGraph,
    registry: &RegistrySnapshot,
    writes: &[CandidateIdentity],
    existing: &Value,
    supplied: &[(String, Value)],
) -> Result<Value, Error> {
    let mut values = existing
        .as_object()
        .cloned()
        .ok_or_else(|| failure("Parameters must be an object"))?;
    let mut seen = BTreeSet::new();
    for (selector, value) in supplied {
        let (reference, name) = selector
            .rsplit_once('#')
            .map_or((None, selector.as_str()), |(r, n)| (Some(r), n));
        let identity = reference
            .map(|r| {
                registry
                    .resolve_output(r)
                    .map(|d| CandidateIdentity::Registered(d.key()))
                    .map_err(failure)
            })
            .transpose()?;
        let mut matches = vec![];
        for d in graph.candidate().definitions().iter().filter(|d| {
            writes.contains(&d.output) && identity.as_ref().is_none_or(|id| id == &d.output)
        }) {
            if let Some(p) = d.declaration["parameters"]
                .as_array()
                .and_then(|p| p.iter().find(|p| p["name"] == name))
            {
                matches.push((d, p));
            }
        }
        let first = matches.first().ok_or_else(|| {
            failure("Parameter is unknown or outside the selected producer scope")
        })?;
        if matches
            .iter()
            .any(|(_, p)| p["logical_type"] != first.1["logical_type"])
        {
            return Err(failure(
                "Parameter name has incompatible definitions; qualify it as dataset#name",
            ));
        }
        for (d, p) in matches {
            if !seen.insert((d.output.clone(), name.to_owned())) {
                return Err(failure(
                    "Parameter override is repeated through the same or aliased reference",
                ));
            }
            let typed = scalar(value, &p["logical_type"])?;
            let entry = values
                .entry(d.path.to_string())
                .or_insert_with(|| json!({}));
            if entry
                .as_object_mut()
                .ok_or_else(|| failure("Invalid parameter map"))?
                .insert(name.into(), typed)
                .is_some()
            {
                return Err(failure("Parameter override is repeated"));
            }
        }
    }
    // Resolve aliases once, so duplicate dataset spellings cannot overwrite each other later.
    let mut identities = BTreeMap::new();
    for key in values.keys() {
        if identities
            .insert(registry.resolve_output(key).map_err(failure)?.key(), key)
            .is_some()
        {
            return Err(failure("Parameter dataset is repeated through aliases"));
        }
    }
    Ok(Value::Object(values))
}
fn scalar(value: &Value, logical: &Value) -> Result<Value, Error> {
    let kind = logical["type"]
        .as_str()
        .ok_or_else(|| failure("Invalid parameter type"))?;
    let result = match value {
        Value::Object(_) => value.clone(),
        Value::Null => json!({"type":"null"}),
        Value::Bool(v) if kind == "bool" => json!({"type":"bool","value":v}),
        Value::String(v) if kind == "string" => json!({"type":"string","value":v}),
        Value::Number(v)
            if matches!(
                kind,
                "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64"
            ) && (v.is_i64() || v.is_u64()) =>
        {
            json!({"type":kind,"value":v.to_string()})
        }
        Value::Number(v) if matches!(kind, "f32" | "f64") => {
            json!({"type":kind,"value":v.to_string()})
        }
        _ => {
            return Err(failure(
                "Parameter value does not match its declaration; use a lossless ScalarValue object for complex or large values",
            ));
        }
    };
    tf_protocol::validate_document("ScalarValue", &result).map_err(failure)?;
    Ok(result)
}
