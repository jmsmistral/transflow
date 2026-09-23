//! Closed AST-v1 codec and semantic preflight. Never opens data or executes predicates.
use crate::validate_document;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use tf_domain::{expectation::*, schema::FieldName};

/// Safe structural/semantic explanation, without source values or SQL.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AstError(pub &'static str);
impl std::fmt::Display for AstError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for AstError {}
type Result<T> = std::result::Result<T, AstError>;
fn name(v: &Value) -> Result<FieldName> {
    FieldName::new(
        v.as_str()
            .ok_or(AstError("Expectation name is malformed"))?,
    )
    .map_err(|_| AstError("Expectation name is invalid"))
}
fn comparison(v: &Value) -> Result<Comparison> {
    Ok(match v.as_str() {
        Some("gt") => Comparison::Gt,
        Some("gte") => Comparison::Gte,
        Some("lt") => Comparison::Lt,
        Some("lte") => Comparison::Lte,
        Some("equals") => Comparison::Equals,
        Some("not_equals") => Comparison::NotEquals,
        _ => return Err(AstError("Unsupported comparison operator")),
    })
}
fn column(v: &Value) -> Result<ColumnValue<Value>> {
    match v["kind"].as_str() {
        Some("column") => Ok(ColumnValue::Column(name(&v["name"])?)),
        Some("literal") if v["value"]["type"] != "null" => {
            Ok(ColumnValue::Literal(v["value"].clone()))
        }
        _ => Err(AstError(
            "Use explicit null predicates instead of a null comparison",
        )),
    }
}
fn integer(v: &Value) -> bool {
    matches!(
        v["type"].as_str(),
        Some("i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64")
    )
}
fn scalar(v: &Value, aliases: &BTreeSet<&str>) -> Result<ScalarOperand<Value>> {
    match v["kind"].as_str() {
        Some("row_count") => {
            let source = if v["input"].is_null() {
                None
            } else {
                let alias = name(&v["input"]["alias"])?;
                if !aliases.contains(alias.as_str()) {
                    return Err(AstError("Expectation references an undeclared input alias"));
                }
                Some(InputAliasRef(alias))
            };
            Ok(ScalarOperand::Metric(ScalarMetric::RowCount(source)))
        }
        Some("literal") if integer(&v["value"]) => Ok(ScalarOperand::Literal(v["value"].clone())),
        _ => Err(AstError("Row-count comparisons require an integer literal")),
    }
}
fn row(e: Expectation<Value>) -> Result<RowPredicate<Value>> {
    if let Expectation::Row(e) = e {
        Ok(e)
    } else {
        Err(AstError(
            "Mixed boolean kinds require explicit every promotion",
        ))
    }
}
fn dataset(e: Expectation<Value>) -> Result<DatasetExpectation<Value>> {
    if let Expectation::Dataset(e) = e {
        Ok(e)
    } else {
        Err(AstError(
            "Mixed boolean kinds require explicit every promotion",
        ))
    }
}
fn node(
    v: &Value,
    aliases: &BTreeSet<&str>,
    depth: usize,
    budget: &mut usize,
) -> Result<Expectation<Value>> {
    if depth > 32 || *budget == 0 {
        return Err(AstError("Expectation exceeds its depth or node limit"));
    }
    *budget -= 1;
    let parse = |child: &Value, budget: &mut usize| node(child, aliases, depth + 1, budget);
    Ok(match v["kind"].as_str() {
        Some("non_null" | "primary_key") => {
            let columns = v["columns"]
                .as_array()
                .ok_or(AstError("Expectation columns are malformed"))?
                .iter()
                .map(name)
                .collect::<Result<Vec<_>>>()?;
            if columns.is_empty() || columns.iter().collect::<BTreeSet<_>>().len() != columns.len()
            {
                return Err(AstError("Expectation columns are empty or repeated"));
            }
            if v["kind"] == "non_null" {
                if columns.len() != 1 {
                    return Err(AstError("Non-null requires exactly one column"));
                }
                Expectation::Row(RowPredicate::NonNull(
                    columns
                        .into_iter()
                        .next()
                        .ok_or(AstError("Missing column"))?,
                ))
            } else {
                Expectation::Dataset(DatasetExpectation::PrimaryKey(columns))
            }
        }
        Some("is_null") => Expectation::Row(RowPredicate::IsNull(name(&v["column"])?)),
        Some("is_finite") => Expectation::Row(RowPredicate::IsFinite(name(&v["column"])?)),
        Some("is_nan") => Expectation::Row(RowPredicate::IsNan(name(&v["column"])?)),
        Some("is_in") => {
            let values = v["values"]
                .as_array()
                .ok_or(AstError("Membership values are malformed"))?;
            let types: Vec<_> = values
                .iter()
                .filter(|v| v["type"] != "null")
                .map(value_type)
                .collect();
            if let Some(first) = types.first()
                && types.iter().any(|t| !compatible(first, t))
            {
                return Err(AstError("Membership values have incompatible types"));
            }
            Expectation::Row(RowPredicate::IsIn(name(&v["column"])?, values.clone()))
        }
        Some("exists") => Expectation::Dataset(DatasetExpectation::Exists(name(&v["column"])?)),
        Some("has_type") => Expectation::Dataset(DatasetExpectation::HasType(
            name(&v["column"])?,
            v["logical_type"].clone(),
        )),
        Some("row_compare") => {
            let (left, right) = (column(&v["left"])?, column(&v["right"])?);
            if matches!(
                (&left, &right),
                (ColumnValue::Literal(_), ColumnValue::Literal(_))
            ) {
                return Err(AstError("Comparison requires a column or metric operand"));
            }
            Expectation::Row(RowPredicate::Compare(comparison(&v["op"])?, left, right))
        }
        Some("metric_compare") => {
            let (left, right) = (scalar(&v["left"], aliases)?, scalar(&v["right"], aliases)?);
            if matches!(
                (&left, &right),
                (ScalarOperand::Literal(_), ScalarOperand::Literal(_))
            ) {
                return Err(AstError("Comparison requires a column or metric operand"));
            }
            Expectation::Dataset(DatasetExpectation::Compare(
                comparison(&v["op"])?,
                left,
                right,
            ))
        }
        Some("row_all" | "row_any" | "dataset_all" | "dataset_any") => {
            let children = v["children"]
                .as_array()
                .ok_or(AstError("Boolean children are malformed"))?;
            if children.is_empty() {
                return Err(AstError("Boolean composition requires operands"));
            }
            let op = if matches!(v["kind"].as_str(), Some("row_all" | "dataset_all")) {
                BooleanOp::All
            } else {
                BooleanOp::Any
            };
            let parsed = children
                .iter()
                .map(|c| parse(c, budget))
                .collect::<Result<Vec<_>>>()?;
            if matches!(v["kind"].as_str(), Some("row_all" | "row_any")) {
                Expectation::Row(RowPredicate::Boolean(
                    op,
                    parsed.into_iter().map(row).collect::<Result<_>>()?,
                ))
            } else {
                Expectation::Dataset(DatasetExpectation::Boolean(
                    op,
                    parsed.into_iter().map(dataset).collect::<Result<_>>()?,
                ))
            }
        }
        Some("row_not") => Expectation::Row(RowPredicate::Not(Box::new(row(parse(
            &v["child"],
            budget,
        )?)?))),
        Some("dataset_not") => Expectation::Dataset(DatasetExpectation::Not(Box::new(dataset(
            parse(&v["child"], budget)?,
        )?))),
        Some("every") => {
            Expectation::Dataset(DatasetExpectation::Every(row(parse(&v["child"], budget)?)?))
        }
        _ => return Err(AstError("Unsupported expectation operator")),
    })
}
/// Decode a bounded AST and resolve all cross-input references against declared aliases.
/// Unknown schemas do not prevent structural validation or create a data PASS result.
pub fn decode(value: &Value, aliases: &BTreeSet<&str>) -> Result<Expectation<Value>> {
    validate_document("ExpectationAstV1", value)
        .map_err(|_| AstError("Expectation AST version, operator or fields are unsupported"))?;
    node(value, aliases, 0, &mut 1000)
}
fn value_type(value: &Value) -> Value {
    let mut result = value.clone();
    if let Some(map) = result.as_object_mut() {
        map.remove("value");
    }
    result
}
fn compatible(left: &Value, right: &Value) -> bool {
    !matches!(left["type"].as_str(), Some("list" | "struct"))
        && ((integer(left) && integer(right)) || left == right)
}
fn operand_type(v: &ColumnValue<Value>, fields: &BTreeMap<&str, &Value>) -> Result<Value> {
    match v {
        ColumnValue::Column(name) => {
            fields
                .get(name.as_str())
                .map(|v| (*v).clone())
                .ok_or(AstError(
                    "An expectation names a column absent from the declared schema",
                ))
        }
        ColumnValue::Literal(v) => Ok(value_type(v)),
    }
}
fn check_row(row: &RowPredicate<Value>, fields: &BTreeMap<&str, &Value>) -> Result<()> {
    match row {
        RowPredicate::NonNull(name) | RowPredicate::IsNull(name) => {
            operand_type(&ColumnValue::Column(name.clone()), fields)?;
        }
        RowPredicate::IsFinite(name) | RowPredicate::IsNan(name) => {
            let typ = operand_type(&ColumnValue::Column(name.clone()), fields)?;
            if !matches!(typ["type"].as_str(), Some("f32" | "f64")) {
                return Err(AstError("Float predicates require a floating-point column"));
            }
        }
        RowPredicate::IsIn(name, values) => {
            let typ = operand_type(&ColumnValue::Column(name.clone()), fields)?;
            if matches!(typ["type"].as_str(), Some("list" | "struct")) {
                return Err(AstError("Membership requires a scalar column"));
            }
            if values
                .iter()
                .filter(|v| v["type"] != "null")
                .any(|v| !compatible(&typ, &value_type(v)))
            {
                return Err(AstError(
                    "Membership values have incompatible declared types",
                ));
            }
        }
        RowPredicate::Compare(_, left, right) => {
            if !compatible(&operand_type(left, fields)?, &operand_type(right, fields)?) {
                return Err(AstError(
                    "Expectation comparison operands have incompatible declared types",
                ));
            }
        }
        RowPredicate::Boolean(_, children) => {
            for child in children {
                check_row(child, fields)?;
            }
        }
        RowPredicate::Not(child) => check_row(child, fields)?,
    }
    Ok(())
}
fn check_dataset(e: &DatasetExpectation<Value>, fields: &BTreeMap<&str, &Value>) -> Result<()> {
    match e {
        DatasetExpectation::Exists(_) => (),
        DatasetExpectation::HasType(name, _) => {
            operand_type(&ColumnValue::Column(name.clone()), fields)?;
        }
        DatasetExpectation::PrimaryKey(names) => {
            for name in names {
                let typ = operand_type(&ColumnValue::Column(name.clone()), fields)?;
                if !integer(&typ)
                    && !matches!(
                        typ["type"].as_str(),
                        Some("bool" | "string" | "decimal" | "date" | "timestamp")
                    )
                {
                    return Err(AstError(
                        "Primary-key columns have unsupported declared types",
                    ));
                }
            }
        }
        DatasetExpectation::Every(row) => check_row(row, fields)?,
        DatasetExpectation::Boolean(_, children) => {
            for child in children {
                check_dataset(child, fields)?;
            }
        }
        DatasetExpectation::Not(child) => check_dataset(child, fields)?,
        DatasetExpectation::Compare(..) => (),
    }
    Ok(())
}
/// Check only an available declared schema. Actual staged/pinned schema remains a runtime obligation.
pub fn validate_schema(e: &Expectation<Value>, schema: &Value) -> Result<()> {
    if schema.is_null() {
        return Ok(());
    }
    validate_document("LogicalSchemaV1", schema)
        .map_err(|_| AstError("Declared schema is invalid"))?;
    let fields = schema["fields"]
        .as_array()
        .ok_or(AstError("Declared schema fields are invalid"))?
        .iter()
        .filter_map(|f| f["name"].as_str().map(|name| (name, &f["logical_type"])))
        .collect();
    match e {
        Expectation::Row(row) => check_row(row, &fields),
        Expectation::Dataset(e) => check_dataset(e, &fields),
    }
}

fn comparison_name(op: Comparison) -> &'static str {
    match op {
        Comparison::Gt => "gt",
        Comparison::Gte => "gte",
        Comparison::Lt => "lt",
        Comparison::Lte => "lte",
        Comparison::Equals => "equals",
        Comparison::NotEquals => "not_equals",
    }
}
fn column_wire(v: &ColumnValue<Value>) -> Value {
    match v {
        ColumnValue::Column(name) => serde_json::json!({"kind":"column","name":name.as_str()}),
        ColumnValue::Literal(value) => serde_json::json!({"kind":"literal","value":value}),
    }
}
fn scalar_wire(v: &ScalarOperand<Value>) -> Value {
    match v {
        ScalarOperand::Literal(value) => serde_json::json!({"kind":"literal","value":value}),
        ScalarOperand::Metric(ScalarMetric::RowCount(source)) => {
            serde_json::json!({"kind":"row_count","input":source.as_ref().map(|s|serde_json::json!({"kind":"input","alias":s.0.as_str()}))})
        }
    }
}
fn row_wire(v: &RowPredicate<Value>) -> Value {
    use serde_json::json;
    match v {
        RowPredicate::NonNull(name) => json!({"kind":"non_null","columns":[name.as_str()]}),
        RowPredicate::IsNull(name) => json!({"kind":"is_null","column":name.as_str()}),
        RowPredicate::IsFinite(name) => json!({"kind":"is_finite","column":name.as_str()}),
        RowPredicate::IsNan(name) => json!({"kind":"is_nan","column":name.as_str()}),
        RowPredicate::IsIn(name, values) => {
            json!({"kind":"is_in","column":name.as_str(),"values":values})
        }
        RowPredicate::Compare(op, l, r) => {
            json!({"kind":"row_compare","op":comparison_name(*op),"left":column_wire(l),"right":column_wire(r)})
        }
        RowPredicate::Boolean(op, children) => {
            json!({"kind":if *op==BooleanOp::All {"row_all"} else {"row_any"},"children":children.iter().map(row_wire).collect::<Vec<_>>()})
        }
        RowPredicate::Not(child) => json!({"kind":"row_not","child":row_wire(child)}),
    }
}
fn dataset_wire(v: &DatasetExpectation<Value>) -> Value {
    use serde_json::json;
    match v {
        DatasetExpectation::Exists(name) => json!({"kind":"exists","column":name.as_str()}),
        DatasetExpectation::HasType(name, logical) => {
            json!({"kind":"has_type","column":name.as_str(),"logical_type":logical})
        }
        DatasetExpectation::PrimaryKey(names) => {
            json!({"kind":"primary_key","columns":names.iter().map(FieldName::as_str).collect::<Vec<_>>()})
        }
        DatasetExpectation::Compare(op, l, r) => {
            json!({"kind":"metric_compare","op":comparison_name(*op),"left":scalar_wire(l),"right":scalar_wire(r)})
        }
        DatasetExpectation::Every(row) => json!({"kind":"every","child":row_wire(row)}),
        DatasetExpectation::Boolean(op, children) => {
            json!({"kind":if *op==BooleanOp::All {"dataset_all"} else {"dataset_any"},"children":children.iter().map(dataset_wire).collect::<Vec<_>>()})
        }
        DatasetExpectation::Not(child) => json!({"kind":"dataset_not","child":dataset_wire(child)}),
    }
}
enum Pending<'a> {
    Row(&'a RowPredicate<Value>),
    Dataset(&'a DatasetExpectation<Value>),
}
fn bounded(value: &Expectation<Value>) -> Result<()> {
    let root = match value {
        Expectation::Row(r) => Pending::Row(r),
        Expectation::Dataset(d) => Pending::Dataset(d),
    };
    let mut pending = vec![(root, 0)];
    let mut count = 0;
    while let Some((node, depth)) = pending.pop() {
        count += 1;
        if depth > 32 || count > 1000 {
            return Err(AstError("Expectation exceeds its depth or node limit"));
        }
        match node {
            Pending::Row(RowPredicate::Not(c)) => pending.push((Pending::Row(c), depth + 1)),
            Pending::Row(RowPredicate::Boolean(_, children)) => {
                if children.len() > 1000 {
                    return Err(AstError("Too many boolean operands"));
                }
                pending.extend(children.iter().map(|c| (Pending::Row(c), depth + 1)));
            }
            Pending::Dataset(DatasetExpectation::Not(c)) => {
                pending.push((Pending::Dataset(c), depth + 1))
            }
            Pending::Dataset(DatasetExpectation::Every(c)) => {
                pending.push((Pending::Row(c), depth + 1))
            }
            Pending::Dataset(DatasetExpectation::Boolean(_, children)) => {
                if children.len() > 1000 {
                    return Err(AstError("Too many boolean operands"));
                }
                pending.extend(children.iter().map(|c| (Pending::Dataset(c), depth + 1)));
            }
            _ => (),
        }
    }
    Ok(())
}
/// Encode typed syntax to the canonical AST-v1 shape, revalidating caller-built trees.
pub fn encode(value: &Expectation<Value>, aliases: &BTreeSet<&str>) -> Result<Value> {
    bounded(value)?;
    let mut result = match value {
        Expectation::Row(r) => row_wire(r),
        Expectation::Dataset(d) => dataset_wire(d),
    };
    result["ast_version"] = 1.into();
    decode(&result, aliases)?;
    Ok(result)
}
