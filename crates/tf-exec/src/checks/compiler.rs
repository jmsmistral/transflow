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
    Count(Comparison, Option<i128>, Option<i128>, usize),
    Boolean(BooleanOp, Vec<Self>),
    Not(Box<Self>),
    Every(Box<Self>),
}
pub(super) struct Plan {
    pub queries: Vec<Value>,
    pub nodes: Vec<Result<Node>>,
}
#[derive(Clone)]
struct Compiler {
    fields: BTreeMap<String, Value>,
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
                fn scalar(v: &ScalarOperand<Value>) -> Result<Option<i128>> {
                    match v {
                        ScalarOperand::Metric(ScalarMetric::RowCount(None)) => Ok(None),
                        ScalarOperand::Metric(_) => Err("unsupported_cross_input"),
                        ScalarOperand::Literal(v) => v["value"]
                            .as_str()
                            .ok_or("literal")?
                            .parse()
                            .map(Some)
                            .map_err(|_| "literal"),
                    }
                }
                let (a, b) = (scalar(a)?, scalar(b)?);
                // A constant true predicate shares the count scan with row checks.
                let (q, _) = self.slot(&RowPredicate::Boolean(BooleanOp::All, vec![]))?;
                Node::Count(*op, a, b, q)
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
    let fields = schema["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| Some((f["name"].as_str()?.into(), f["logical_type"].clone())))
        .collect();
    let mut c = Compiler {
        fields,
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
            let ast = decode(&check["expectation"], &Default::default()).map_err(|_| "ast")?;
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
    // A trailing reserved batch is unused; references always point to filled queries.
    if c.queries.last() == Some(&Value::Null) {
        c.queries.pop();
    }
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
        let mut c = Compiler { fields:fields.iter().filter_map(|f|Some((f["name"].as_str()?.into(),f["logical_type"].clone()))).collect(),queries:vec![],expressions:vec![],parameters:vec![],batch:0,remaining:4096 };
        let ast = decode(&check["expectation"], &Default::default()).ok()?;
        validate_schema(&ast,schema).ok()?;
        let select = columns.iter().map(|f| {
            let name = quote(f["name"].as_str().unwrap_or(""));
            if f["logical_type"]["type"] == "binary" {format!("to_base64({name})")} else {format!("CAST({name} AS VARCHAR)")}
        }).collect::<Vec<_>>();
        let mut source = "subject".to_owned();
        let predicate = match ast {
            Expectation::Row(r) | Expectation::Dataset(DatasetExpectation::Every(r)) => {
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
