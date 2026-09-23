//! Convert bounded exact aggregate cells into canonical outcomes; no samples or data rows.
use super::compiler::{Node, Result};
use serde_json::{Value, json};
use tf_domain::expectation::{BooleanOp, Comparison};

fn count(rows: &[Value], q: usize, c: usize) -> Result<u64> {
    rows.get(q)
        .filter(|v| v["error"].is_null())
        .and_then(|v| v["counts"].get(c))
        .and_then(Value::as_str)
        .and_then(|v| v.parse().ok())
        .ok_or("query_error")
}
fn comparison(op: Comparison, a: i128, b: i128) -> bool {
    match op {
        Comparison::Gt => a > b,
        Comparison::Gte => a >= b,
        Comparison::Lt => a < b,
        Comparison::Lte => a <= b,
        Comparison::Equals => a == b,
        Comparison::NotEquals => a != b,
    }
}
/// Root row attribution is absent for dataset conditions, including dataset composition.
pub(super) fn outcome(
    node: &Node,
    rows: &[Value],
    ignore: bool,
    path: &str,
    metrics: &mut Vec<Value>,
) -> Result<(bool, Option<u64>)> {
    let mut values = serde_json::Map::new();
    let (pass, failed) = match node {
        Node::Row(q, s, children) => {
            let total = count(rows, *q, 0)?;
            let bad = count(rows, *q, 1 + 2 * s)?;
            let unknown = count(rows, *q, 2 + 2 * s)?;
            if bad.checked_add(unknown).is_none_or(|n| n > total) {
                return Err("result_counts");
            }
            let failed = if ignore { bad } else { bad + unknown };
            values.insert("rows_evaluated".into(), json!(total.to_string()));
            values.insert("unknown_rows".into(), json!(unknown.to_string()));
            values.insert(
                "skipped_rows".into(),
                json!((if ignore { unknown } else { 0 }).to_string()),
            );
            values.insert("violating_rows".into(), json!(failed.to_string()));
            for (i, c) in children.iter().enumerate() {
                outcome(c, rows, ignore, &format!("{path}/{i}"), metrics)?;
            }
            (failed == 0, Some(failed))
        }
        Node::Key(q, s, k) => {
            let total = count(rows, *q, 0)?;
            let nulls = count(rows, *q, 1 + 2 * s)?;
            let groups = count(rows, *k, 0)?;
            let duplicates = count(rows, *k, 1)?;
            let failed = nulls.checked_add(duplicates).ok_or("result_counts")?;
            if failed > total
                || groups.checked_mul(2).is_none_or(|n| n > duplicates)
                || (groups == 0 && duplicates != 0)
            {
                return Err("result_counts");
            }
            for (name, n) in [
                ("null_key_rows", nulls),
                ("duplicate_groups", groups),
                ("duplicate_group_rows", duplicates),
                ("violating_rows", failed),
            ] {
                values.insert(name.into(), json!(n.to_string()));
            }
            (failed == 0, Some(failed))
        }
        Node::Fixed(v) => (*v, None),
        Node::Count(op, a, b, q) => {
            let n = i128::from(count(rows, *q, 0)?);
            let (a, b) = (a.unwrap_or(n), b.unwrap_or(n));
            values.insert("left".into(), json!(a.to_string()));
            values.insert("right".into(), json!(b.to_string()));
            (comparison(*op, a, b), None)
        }
        Node::Boolean(op, children) => {
            let mut results = vec![];
            // Do not short circuit: independent errors remain blocking under any/not/WARN.
            for (i, c) in children.iter().enumerate() {
                results.push(outcome(c, rows, ignore, &format!("{path}/{i}"), metrics)?.0);
            }
            (
                if *op == BooleanOp::All {
                    results.iter().all(|v| *v)
                } else {
                    results.iter().any(|v| *v)
                },
                None,
            )
        }
        Node::Not(c) => (
            !outcome(c, rows, ignore, &format!("{path}/0"), metrics)?.0,
            None,
        ),
        Node::Every(c) => {
            let (p, f) = outcome(c, rows, ignore, &format!("{path}/0"), metrics)?;
            (p, f)
        }
    };
    metrics.push(json!({"path":path,"passed":pass,"counts":values,"expected":if failed.is_some(){json!({"violating_rows":"0"})}else{json!({"passed":"true"})}}));
    Ok((pass, failed))
}
