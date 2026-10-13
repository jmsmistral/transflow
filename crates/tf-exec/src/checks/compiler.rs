//! Application-only SQL generation. Identifiers are quoted; scalar text is always bound.
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tf_domain::expectation::*;
use tf_protocol::expectation::{decode, validate_schema};

pub(super) type Result<T> = std::result::Result<T, &'static str>;
#[derive(Clone)]
pub(super) enum Node {
    Row(usize, usize, Vec<Self>),
    Key(usize, usize, usize),
    Fixed(bool),
    Scalar(Comparison, ScalarPlan, ScalarPlan),
    Group(usize),
    Boolean(BooleanOp, Vec<Self>),
    Not(Box<Self>),
    Every(Box<Self>),
}
#[derive(Clone)]
pub(super) enum ScalarPlan {
    Literal(Value),
    Metric(usize, bool),
}
impl Node {
    fn remap(&mut self, indices: &BTreeMap<usize, usize>) -> Result<()> {
        let index = |value: &mut usize| -> Result<()> {
            *value = *indices.get(value).ok_or("plan_index")?;
            Ok(())
        };
        match self {
            Self::Row(q, _, children) => {
                index(q)?;
                for child in children {
                    child.remap(indices)?;
                }
            }
            Self::Key(q, _, key) => {
                index(q)?;
                index(key)?;
            }
            Self::Scalar(_, a, b) => {
                for operand in [a, b] {
                    if let ScalarPlan::Metric(q, _) = operand {
                        index(q)?;
                    }
                }
            }
            Self::Group(q) => index(q)?,
            Self::Boolean(_, children) => {
                for child in children {
                    child.remap(indices)?;
                }
            }
            Self::Not(child) | Self::Every(child) => child.remap(indices)?,
            Self::Fixed(_) => (),
        }
        Ok(())
    }
}
pub(super) struct Plan {
    pub queries: Vec<Value>,
    pub nodes: Vec<Result<Node>>,
}
#[derive(Clone)]
struct Compiler {
    fields: BTreeMap<String, Value>,
    schema: Value,
    inputs: BTreeMap<String, Value>,
    evaluation_time: Value,
    queries: Vec<Value>,
    expressions: Vec<String>,
    parameters: Vec<Value>,
    batch: usize,
    remaining: usize,
}
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
fn operator(op: Comparison) -> &'static str {
    match op {
        Comparison::Gt => ">",
        Comparison::Gte => ">=",
        Comparison::Lt => "<",
        Comparison::Lte => "<=",
        Comparison::Equals => "=",
        Comparison::NotEquals => "<>",
    }
}
fn integer_type(value: &Value) -> bool {
    matches!(
        value["type"].as_str(),
        Some("i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64")
    )
}
fn numeric_row(row: &RowPredicate<Value>) -> bool {
    match row {
        RowPredicate::Approx(..) | RowPredicate::Age(..) => true,
        RowPredicate::Boolean(_, children) => children.iter().any(numeric_row),
        RowPredicate::Not(child) => numeric_row(child),
        _ => false,
    }
}
// Reviewed RE2 subset: ASCII literals, dot, anchors, classes, plain groups,
// alternation and greedy ?, * and +. No flags, backreferences or lookaround.
fn regex_subset(pattern: &str) -> Result<()> {
    if !pattern.is_ascii() || pattern.len() > 1024 {
        return Err("unsupported_regex");
    }
    let mut group = 0u32;
    let mut class = false;
    let mut class_tokens: Vec<Option<char>> = vec![];
    let mut escaped = false;
    let mut atom = false;
    let mut quantified = false;
    for c in pattern.chars() {
        if escaped {
            if !"dDsSwWnrt\\.^$|?*+()[]-".contains(c) {
                return Err("unsupported_regex");
            }
            if class {
                class_tokens.push(None);
            }
            escaped = false;
            atom = true;
            quantified = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if class {
            if c == ']' {
                let body = if class_tokens.first() == Some(&Some('^')) {
                    &class_tokens[1..]
                } else {
                    class_tokens.as_slice()
                };
                if body.is_empty() {
                    return Err("unsupported_regex");
                }
                for (index, token) in body.iter().enumerate() {
                    if *token == Some('-') && index > 0 && index + 1 < body.len() {
                        match (body[index - 1], body[index + 1]) {
                            (Some(start), Some(end)) if start <= end => (),
                            _ => return Err("unsupported_regex"),
                        }
                    }
                }
                class = false;
                atom = true;
            } else if c == '[' {
                return Err("unsupported_regex");
            } else {
                class_tokens.push(Some(c));
            }
            continue;
        }
        match c {
            '[' => {
                class = true;
                class_tokens.clear();
                quantified = false;
            }
            '(' => {
                group += 1;
                atom = false;
                quantified = false;
            }
            ')' if group > 0 => {
                group -= 1;
                atom = true;
                quantified = false;
            }
            ')' | ']' | '{' | '}' => return Err("unsupported_regex"),
            '*' | '+' | '?' => {
                if !atom || quantified {
                    return Err("unsupported_regex");
                }
                quantified = true;
            }
            '|' => {
                atom = false;
                quantified = false;
            }
            '^' | '$' => {
                atom = false;
                quantified = false;
            }
            _ => {
                atom = true;
                quantified = false;
            }
        }
    }
    if group != 0 || class || escaped {
        return Err("unsupported_regex");
    }
    Ok(())
}
/// Reject known lossy engine mappings, including DuckDB's case-insensitive column lookup.
pub(super) fn schema_supported(schema: &Value) -> Result<()> {
    fn fields(fields: &[Value]) -> Result<()> {
        let mut names = std::collections::BTreeSet::new();
        for f in fields {
            let name = f["name"].as_str().ok_or("schema")?;
            if !names.insert(name.to_ascii_lowercase()) {
                return Err("schema_case_collision");
            }
            typ(&f["logical_type"])?;
        }
        Ok(())
    }
    fn typ(t: &Value) -> Result<()> {
        match t["type"].as_str() {
            Some("decimal")
                if t["precision"].as_u64().is_none_or(|n| n > 38)
                    || t["scale"]
                        .as_i64()
                        .is_none_or(|n| n < 0 || Some(n as u64) > t["precision"].as_u64()) =>
            {
                return Err("unsupported_precision");
            }
            Some("timestamp")
                if t["unit"] == "s"
                    || (!t["timezone"].is_null()
                        && (t["timezone"] != "UTC" || t["unit"] == "ns")) =>
            {
                return Err("unsupported_precision");
            }
            Some("list") => typ(&t["element"]["logical_type"])?,
            Some("struct") => fields(t["fields"].as_array().ok_or("schema")?)?,
            _ => (),
        }
        Ok(())
    }
    fields(schema["fields"].as_array().ok_or("schema")?)
}
impl Compiler {
    fn exact_row(&mut self, row: &RowPredicate<Value>, columns: &mut Vec<String>) -> Result<Value> {
        Ok(match row {
            RowPredicate::Approx(name, value, absolute, relative) => {
                let typ = self.fields.get(name.as_str()).ok_or("missing_column")?;
                let float = matches!(typ["type"].as_str(), Some("f32" | "f64"));
                let column = columns.len();
                let value_column = quote(name.as_str());
                columns.push(if float {
                    value_column
                } else {
                    format!("CAST({value_column} AS VARCHAR)")
                });
                json!({"kind":if float {"approx_float"} else {"approx_decimal"},"column":column,"value":value,"abs_tol":absolute,"rel_tol":relative})
            }
            RowPredicate::Age(op, name, value) => {
                if *op != Comparison::Lte
                    || self.evaluation_time["type"] != "timestamp"
                    || self.evaluation_time["timezone"] != "UTC"
                {
                    return Err("evaluation_time");
                }
                let clock = self.literal(&self.evaluation_time.clone())?;
                let column = columns.len();
                columns.push(format!("CAST((CAST(epoch_us({clock}) AS HUGEINT)-CAST(epoch_us({}) AS HUGEINT)) AS VARCHAR)",quote(name.as_str())));
                json!({"kind":"age","column":column,"value":value})
            }
            RowPredicate::Boolean(op, children) => {
                json!({"kind":if *op == BooleanOp::All {"all"} else {"any"},"children":children.iter().map(|c|self.exact_row(c,columns)).collect::<Result<Vec<_>>>()?})
            }
            RowPredicate::Not(child) => {
                json!({"kind":"not","child":self.exact_row(child,columns)?})
            }
            _ => {
                let column = columns.len();
                columns.push(self.row_sql(row)?);
                json!({"kind":"boolean","column":column})
            }
        })
    }
    fn scalar(&mut self, operand: &ScalarOperand<Value>) -> Result<ScalarPlan> {
        let metric = match operand {
            ScalarOperand::Literal(value) => return Ok(ScalarPlan::Literal(value.clone())),
            ScalarOperand::Metric(metric) => metric,
        };
        let mut fraction = false;
        let (aggregate, view): (String, String) = match metric {
            ScalarMetric::RowCount(None) => ("count(*)".into(), "subject".into()),
            ScalarMetric::RowCount(Some(alias)) => {
                let input = self
                    .inputs
                    .get(alias.0.as_str())
                    .ok_or("undeclared_input")?;
                schema_supported(&input["schema"])?;
                (
                    "count(*)".into(),
                    quote(input["view"].as_str().ok_or("input_view")?),
                )
            }
            ScalarMetric::NullCount(name) | ScalarMetric::NullFraction(name) => {
                fraction = matches!(metric, ScalarMetric::NullFraction(_));
                (
                    format!(
                        "count(*) FILTER (WHERE {} IS NULL){}",
                        quote(name.as_str()),
                        if fraction { ",count(*)" } else { "" }
                    ),
                    "subject".into(),
                )
            }
            ScalarMetric::DistinctCount(name) => (
                format!("count(DISTINCT {})", quote(name.as_str())),
                "subject".into(),
            ),
        };
        let index = self.queries.len();
        self.queries.push(json!({"sql":format!("SELECT {aggregate} FROM {view}"),"parameters":[],"width":if fraction {2} else {1}}));
        Ok(ScalarPlan::Metric(index, fraction))
    }
    fn flush(&mut self) {
        if self.expressions.is_empty() {
            return;
        }
        let aggregates = self
            .expressions
            .iter()
            .flat_map(|e| {
                [
                    format!("count(*) FILTER (WHERE ({e}) IS FALSE)"),
                    format!("count(*) FILTER (WHERE ({e}) IS NULL)"),
                ]
            })
            .collect::<Vec<_>>()
            .join(",");
        self.queries[self.batch] = json!({"sql":format!("SELECT count(*),{aggregates} FROM subject"),"parameters":self.parameters,"width":1+self.expressions.len()*2});
        self.expressions.clear();
        self.parameters.clear();
        self.batch = self.queries.len();
        self.queries.push(Value::Null);
    }
    fn slot(&mut self, row: &RowPredicate<Value>) -> Result<(usize, usize)> {
        if self.expressions.len() >= 32 {
            self.flush();
        }
        self.remaining = self.remaining.checked_sub(1).ok_or("plan_limit")?;
        if numeric_row(row) {
            let saved = std::mem::take(&mut self.parameters);
            let mut columns = vec![];
            let plan = self.exact_row(row, &mut columns)?;
            let parameters = std::mem::replace(&mut self.parameters, saved);
            let index = self.queries.len();
            self.queries.push(json!({"sql":format!("SELECT {} FROM subject",columns.join(",")),"parameters":parameters,"width":3,"row_plan":plan,"row_columns":columns.len()}));
            return Ok((index, 0));
        }
        let expr = self.row_sql(row)?;
        let slot = self.expressions.len();
        self.expressions.push(expr);
        Ok((self.batch, slot))
    }
    fn literal(&mut self, v: &Value) -> Result<String> {
        let kind = v["type"].as_str().ok_or("literal")?;
        let sql_type = match kind {
            "bool" => "BOOLEAN".into(),
            "i8" => "TINYINT".into(),
            "i16" => "SMALLINT".into(),
            "i32" => "INTEGER".into(),
            "i64" => "BIGINT".into(),
            "u8" => "UTINYINT".into(),
            "u16" => "USMALLINT".into(),
            "u32" => "UINTEGER".into(),
            "u64" => "UBIGINT".into(),
            "f32" => "FLOAT".into(),
            "f64" => "DOUBLE".into(),
            "string" => "VARCHAR".into(),
            "date" => "DATE".into(),
            "decimal" => {
                let p = v["precision"].as_u64().ok_or("literal")?;
                let s = v["scale"].as_u64().ok_or("unsupported_precision")?;
                if p > 38 || s > p {
                    return Err("unsupported_precision");
                }
                format!("DECIMAL({p},{s})")
            }
            "timestamp" => {
                if v["timezone"].is_null() {
                    match v["unit"].as_str() {
                        Some("ms") => "TIMESTAMP_MS",
                        Some("us") => "TIMESTAMP",
                        Some("ns") => "TIMESTAMP_NS",
                        _ => return Err("unsupported_precision"),
                    }
                    .into()
                } else if v["timezone"] == "UTC" && v["unit"] != "ns" {
                    "TIMESTAMPTZ".into()
                } else {
                    return Err("unsupported_precision");
                }
            }
            "binary" => {
                self.parameters.push(v.clone());
                return Ok(format!("from_base64(${})", self.parameters.len()));
            }
            _ => return Err("literal"),
        };
        self.parameters.push(v.clone());
        Ok(format!("CAST(${} AS {sql_type})", self.parameters.len()))
    }
    fn operand(&mut self, v: &ColumnValue<Value>) -> Result<(String, bool)> {
        match v {
            ColumnValue::Column(n) => Ok((
                quote(n.as_str()),
                matches!(
                    self.fields.get(n.as_str()).ok_or("missing_column")?["type"].as_str(),
                    Some("f32" | "f64")
                ),
            )),
            ColumnValue::Literal(v) => Ok((
                self.literal(v)?,
                matches!(v["type"].as_str(), Some("f32" | "f64")),
            )),
        }
    }
    fn comparison(
        &mut self,
        op: Comparison,
        a: &ColumnValue<Value>,
        b: &ColumnValue<Value>,
    ) -> Result<String> {
        let (a, af) = self.operand(a)?;
        let (b, bf) = self.operand(b)?;
        let plain = format!("({a} {} {b})", operator(op));
        if af || bf {
            Ok(format!(
                "(CASE WHEN {a} IS NULL OR {b} IS NULL THEN NULL WHEN isnan({a}) OR isnan({b}) THEN {} ELSE {plain} END)",
                if op == Comparison::NotEquals {
                    "TRUE"
                } else {
                    "FALSE"
                }
            ))
        } else {
            Ok(plain)
        }
    }
    fn row_sql(&mut self, r: &RowPredicate<Value>) -> Result<String> {
        Ok(match r {
            RowPredicate::Approx(..) | RowPredicate::Age(..) => {
                return Err("numeric_plan_required");
            }
            RowPredicate::Matches(name, pattern, full) => {
                regex_subset(pattern)?;
                let pattern = self.literal(&json!({"type":"string","value":pattern}))?;
                format!(
                    "{}({}, {pattern}, 'c')",
                    if *full {
                        "regexp_full_match"
                    } else {
                        "regexp_matches"
                    },
                    quote(name.as_str())
                )
            }
            RowPredicate::InDataset(name, alias, foreign) => {
                let input = self
                    .inputs
                    .get(alias.0.as_str())
                    .ok_or("undeclared_input")?;
                schema_supported(&input["schema"])?;
                let fields = input["schema"]["fields"].as_array().ok_or("input_schema")?;
                let remote = fields
                    .iter()
                    .find(|f| f["name"] == foreign.as_str())
                    .ok_or("missing_input_column")?;
                let local = self.fields.get(name.as_str()).ok_or("missing_column")?;
                if !(local == &remote["logical_type"]
                    || integer_type(local) && integer_type(&remote["logical_type"]))
                    || matches!(local["type"].as_str(), Some("list" | "struct"))
                {
                    return Err("input_type");
                }
                let lhs = format!("subject.{}", quote(name.as_str()));
                let rhs = format!("reference.{}", quote(foreign.as_str()));
                let view = quote(input["view"].as_str().ok_or("input_view")?);
                let comparison = if matches!(local["type"].as_str(), Some("f32" | "f64")) {
                    format!("{rhs} = {lhs} AND NOT isnan({rhs}) AND NOT isnan({lhs})")
                } else {
                    format!("{rhs} = {lhs}")
                };
                format!(
                    "(CASE WHEN {lhs} IS NULL THEN NULL ELSE EXISTS(SELECT 1 FROM {view} AS reference WHERE {comparison}) END)"
                )
            }
            RowPredicate::NonNull(n) => format!("({} IS NOT NULL)", quote(n.as_str())),
            RowPredicate::IsNull(n) => format!("({} IS NULL)", quote(n.as_str())),
            RowPredicate::IsFinite(n) => format!("isfinite({})", quote(n.as_str())),
            RowPredicate::IsNan(n) => format!("isnan({})", quote(n.as_str())),
            RowPredicate::Compare(op, a, b) => self.comparison(*op, a, b)?,
            RowPredicate::IsIn(n, vs) => {
                let mut terms = vec![];
                for v in vs {
                    terms.push(if v["type"] == "null" {
                        format!("({} IS NULL)", quote(n.as_str()))
                    } else {
                        format!(
                            "coalesce({},FALSE)",
                            self.comparison(
                                Comparison::Equals,
                                &ColumnValue::Column(n.clone()),
                                &ColumnValue::Literal(v.clone())
                            )?
                        )
                    });
                }
                if terms.is_empty() {
                    "FALSE".into()
                } else {
                    format!("({})", terms.join(" OR "))
                }
            }
            RowPredicate::Boolean(_, children) if children.is_empty() => "TRUE".into(),
            RowPredicate::Boolean(op, children) => {
                let parts = children
                    .iter()
                    .map(|c| self.row_sql(c))
                    .collect::<Result<Vec<_>>>()?;
                format!(
                    "({})",
                    parts.join(if *op == BooleanOp::All {
                        " AND "
                    } else {
                        " OR "
                    })
                )
            }
            RowPredicate::Not(c) => format!("(NOT {})", self.row_sql(c)?),
        })
    }
    fn row_node(&mut self, r: &RowPredicate<Value>) -> Result<Node> {
        let (q, s) = self.slot(r)?;
        let children = match r {
            RowPredicate::Boolean(_, cs) => cs
                .iter()
                .map(|c| self.row_node(c))
                .collect::<Result<Vec<_>>>()?,
            RowPredicate::Not(c) => vec![self.row_node(c)?],
            _ => vec![],
        };
        Ok(Node::Row(q, s, children))
    }
    fn visit(&mut self, e: &DatasetExpectation<Value>) -> Result<Node> {
        self.remaining = self.remaining.checked_sub(1).ok_or("plan_limit")?;
        Ok(match e {
            DatasetExpectation::SchemaEquals(schema) => Node::Fixed(&self.schema == schema),
            DatasetExpectation::GroupCount(op, names, value) => {
                let index = self.queries.len();
                let before = self.parameters.len();
                let bound = self.literal(value)?;
                let parameters = self.parameters.split_off(before);
                let bound = bound.replace(&format!("${}", before + 1), "$1");
                let columns = names
                    .iter()
                    .map(|n| quote(n.as_str()))
                    .collect::<Vec<_>>()
                    .join(",");
                self.queries.push(json!({"sql":format!("SELECT count(*),count(*) FILTER (WHERE NOT(n {} {bound})) FROM (SELECT count(*) AS n FROM subject GROUP BY {columns}) AS groups" ,operator(*op)),"parameters":parameters,"width":2}));
                Node::Group(index)
            }
            DatasetExpectation::Exists(n) => Node::Fixed(self.fields.contains_key(n.as_str())),
            DatasetExpectation::HasType(n, t) => {
                Node::Fixed(self.fields.get(n.as_str()) == Some(t))
            }
            DatasetExpectation::Every(r) => Node::Every(Box::new(self.row_node(r)?)),
            DatasetExpectation::PrimaryKey(names) => {
                let nulls = RowPredicate::Boolean(
                    BooleanOp::Any,
                    names.iter().cloned().map(RowPredicate::IsNull).collect(),
                );
                // False count of non-null condition gives null-key rows; never ignored.
                let (q, s) = self.slot(&RowPredicate::Not(Box::new(nulls)))?;
                let cols = names.iter().map(|n| quote(n.as_str())).collect::<Vec<_>>();
                let nonnull = cols
                    .iter()
                    .map(|n| format!("{n} IS NOT NULL"))
                    .collect::<Vec<_>>()
                    .join(" AND ");
                let index = self.queries.len();
                self.queries.push(json!({"sql":format!("SELECT count(*) FILTER (WHERE n>1),coalesce(sum(n) FILTER (WHERE n>1),0) FROM (SELECT count(*) AS n FROM subject WHERE {nonnull} GROUP BY {}) AS key_groups",cols.join(",")),"parameters":[],"width":2}));
                Node::Key(q, s, index)
            }
            DatasetExpectation::Compare(op, a, b) => {
                Node::Scalar(*op, self.scalar(a)?, self.scalar(b)?)
            }
            DatasetExpectation::Boolean(op, children) => Node::Boolean(
                *op,
                children
                    .iter()
                    .map(|c| self.visit(c))
                    .collect::<Result<Vec<_>>>()?,
            ),
            DatasetExpectation::Not(c) => Node::Not(Box::new(self.visit(c)?)),
        })
    }
}
pub(super) fn compile(checks: &[Value], schema: &Value) -> Plan {
    compile_bound(checks, schema, &BTreeMap::new(), &Value::Null)
}
pub(super) fn compile_bound(
    checks: &[Value],
    schema: &Value,
    inputs: &BTreeMap<String, Value>,
    clock: &Value,
) -> Plan {
    let fields = schema["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| Some((f["name"].as_str()?.into(), f["logical_type"].clone())))
        .collect();
    let mut c = Compiler {
        fields,
        schema: schema.clone(),
        inputs: inputs.clone(),
        evaluation_time: clock.clone(),
        queries: vec![Value::Null],
        expressions: vec![],
        parameters: vec![],
        batch: 0,
        remaining: 4096,
    };
    let mut nodes = vec![];
    for check in checks {
        let checkpoint = c.clone();
        let result = (|| {
            let ast = decode(
                &check["expectation"],
                &inputs.keys().map(String::as_str).collect(),
            )
            .map_err(|_| "ast")?;
            validate_schema(&ast, schema).map_err(|_| "type_or_column")?;
            let node = match ast {
                Expectation::Row(r) => c.row_node(&r)?,
                Expectation::Dataset(d) => c.visit(&d)?,
            };
            Ok(node)
        })();
        if result.is_err() {
            c = checkpoint;
        }
        nodes.push(result);
    }
    c.flush();
    // Scalar-only plans leave the row batch unused. Remove every reservation and
    // remap typed references, preserving fixed-only plans without a worker scan.
    let indices = c
        .queries
        .iter()
        .enumerate()
        .filter(|(_, q)| !q.is_null())
        .enumerate()
        .map(|(new, (old, _))| (old, new))
        .collect();
    for node in &mut nodes {
        if let Ok(value) = node
            && let Err(error) = value.remap(&indices)
        {
            *node = Err(error);
        }
    }
    c.queries.retain(|q| !q.is_null());
    Plan {
        queries: c.queries,
        nodes,
    }
}

/// Compile independent, bounded diagnostic reads. They never decide the aggregate outcome.
pub(super) fn samples(checks: &[Value], schema: &Value, policy: &Value) -> Vec<Option<Value>> {
    let fields = schema["fields"].as_array().cloned().unwrap_or_default();
    let sensitive = policy["sensitive_columns"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let columns: Vec<_> = policy["allowed_columns"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| !sensitive.contains(n))
        .filter_map(|n| fields.iter().find(|f| f["name"] == *n))
        .filter(|f| !matches!(f["logical_type"]["type"].as_str(), Some("list" | "struct")))
        .cloned()
        .collect();
    checks.iter().map(|check| {
        let requested = check["sample_rows"].as_str()?.parse::<u64>().ok()?;
        if requested == 0 { return None; }
        let limit = requested.min(policy["max_rows"].as_u64().unwrap_or(20)).min(20);
        let mut c = Compiler { schema:schema.clone(), inputs:BTreeMap::new(), evaluation_time:Value::Null, fields:fields.iter().filter_map(|f|Some((f["name"].as_str()?.into(),f["logical_type"].clone()))).collect(),queries:vec![],expressions:vec![],parameters:vec![],batch:0,remaining:4096 };
        let ast = decode(&check["expectation"], &Default::default()).ok()?;
        validate_schema(&ast,schema).ok()?;
        let select = columns.iter().map(|f| {
            let name = quote(f["name"].as_str().unwrap_or(""));
            if f["logical_type"]["type"] == "binary" {format!("to_base64({name})")} else {format!("CAST({name} AS VARCHAR)")}
        }).collect::<Vec<_>>();
        let mut source = "subject".to_owned();
        let predicate = match ast {
            Expectation::Row(r) | Expectation::Dataset(DatasetExpectation::Every(r)) => {
                if numeric_row(&r) { return None; }
                let sql = c.row_sql(&r).ok()?;
                Some(format!("({sql}) IS {}", if check["null_policy"]=="ignore" {"FALSE"} else {"NOT TRUE"}))
            },
            Expectation::Dataset(DatasetExpectation::PrimaryKey(names)) => {
                let keys = names.iter().map(|n|quote(n.as_str())).collect::<Vec<_>>();
                // An internal name is chosen outside the actual schema namespace.
                let mut name = "__transflow_sample_count".to_owned();
                while c.fields.keys().any(|n|n.eq_ignore_ascii_case(&name)) {name.push('_');}
                let name = quote(&name);
                source = format!("(SELECT *,count(*) OVER (PARTITION BY {}) AS {name} FROM subject) AS sample_source",keys.join(","));
                Some(format!("({} OR {name}>1)",keys.iter().map(|k|format!("{k} IS NULL")).collect::<Vec<_>>().join(" OR ")))
            },
            _ => None,
        };
        let reason = if limit==0 {Some("policy_disabled")} else if columns.is_empty(){Some("no_allowed_columns")} else if predicate.is_none(){Some("no_row_attribution")} else {None};
        let sql = if let Some(predicate)=predicate.filter(|_|reason.is_none()) {
            let bounded=select.iter().map(|s|format!("({s} IS NULL OR octet_length(encode({s}))<=4096)")).collect::<Vec<_>>().join(" AND ");
            format!("SELECT {} FROM {source} WHERE ({predicate}) AND ({bounded}) LIMIT {}",select.join(","),limit+1)
        } else {String::new()};
        Some(json!({"sql":sql,"parameters":if reason.is_some(){vec![]}else{c.parameters},"columns":columns,"limit":limit,"reason":reason}))
    }).collect()
}
