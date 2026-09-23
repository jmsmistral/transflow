//! Canonical G1 truth/metric conformance over pure domain kernels, not an artifact evaluator.
#![allow(clippy::unwrap_used, reason = "Synthetic conformance assertions")]
use serde_json::Value;
use std::collections::BTreeMap;
use tf_domain::{
    expectation::{Comparison, semantics::*},
    schema::LogicalType,
    value::*,
};
fn fixtures() -> Value {
    serde_json::from_str(include_str!(
        "../../../schemas/fixtures/expectation-semantics-v1.json"
    ))
    .unwrap()
}
fn truth(v: &Value) -> Truth {
    [
        ("true", Truth::True),
        ("false", Truth::False),
        ("unknown", Truth::Unknown),
    ]
    .into_iter()
    .find(|(name, _)| *name == v.as_str().unwrap())
    .unwrap()
    .1
}
fn integer(n: i64) -> ScalarValue {
    ScalarValue::Integer(IntegerValue::parse(IntegerType::I64, &n.to_string()).unwrap())
}
fn float(n: &str) -> ScalarValue {
    ScalarValue::Float(FloatValue::parse(FloatWidth::F64, n).unwrap())
}
#[test]
fn three_valued_truth_is_composed_before_the_root_policy() {
    let fixtures = fixtures();
    for case in fixtures["truth"].as_array().unwrap() {
        let (a, b) = (truth(&case["left"]), truth(&case["right"]));
        assert_eq!(a.and(b), truth(&case["all"]));
        assert_eq!(a.or(b), truth(&case["any"]));
    }
    for (a, b) in fixtures["not"].as_object().unwrap() {
        assert_eq!(!truth(&Value::String(a.clone())), truth(b));
    }
    for policy in [NullPolicy::Fail, NullPolicy::Ignore] {
        let mut counts = RowMetrics::default();
        for t in [Truth::True, Truth::False, Truth::Unknown] {
            counts.observe(t, policy).unwrap();
        }
        assert_eq!(counts.rows_evaluated, 3);
        assert_eq!(
            counts.violating_rows,
            if policy == NullPolicy::Fail { 2 } else { 1 }
        );
        assert_eq!(
            counts.skipped_rows,
            if policy == NullPolicy::Fail { 0 } else { 1 }
        );
        assert!(!counts.passes());
        assert!(RowMetrics::default().passes());
        let mut composed = RowMetrics::default();
        composed
            .observe(Truth::False.and(Truth::Unknown), policy)
            .unwrap();
        assert_eq!(composed.violating_rows, 1);
        assert_eq!(composed.skipped_rows, 0);
    }
}
#[test]
fn age_boundaries_and_empty_count_have_exact_outcomes() {
    for case in fixtures()["ages"].as_array().unwrap() {
        let value = case["value"].as_i64().map_or(ScalarValue::Null, integer);
        let result = (!is_null(&value))
            .and(compare(Comparison::Gte, &value, &integer(0)).unwrap())
            .and(compare(Comparison::Lt, &value, &integer(200)).unwrap());
        assert_eq!(result, truth(&case["truth"]));
    }
    assert_eq!(
        compare(Comparison::Gt, &integer(0), &integer(0)).unwrap(),
        Truth::False
    );
    assert_eq!(
        compare(Comparison::Equals, &integer(0), &integer(0)).unwrap(),
        Truth::True
    );
}
#[test]
fn primary_keys_count_null_rows_and_all_duplicate_rows_with_real_tuples() {
    for case in fixtures()["keys"].as_array().unwrap() {
        let types: Vec<_> = case["types"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                if t == "i64" {
                    LogicalType::Integer(IntegerType::I64)
                } else {
                    LogicalType::String
                }
            })
            .collect();
        let schema = KeySchema::new(types).unwrap();
        let mut groups = BTreeMap::new();
        let mut metrics = PrimaryKeyMetrics::default();
        for row in case["rows"].as_array().unwrap() {
            let values: Vec<_> = row
                .as_array()
                .unwrap()
                .iter()
                .map(|v| {
                    if v.is_null() {
                        ScalarValue::Null
                    } else if let Some(n) = v.as_i64() {
                        integer(n)
                    } else {
                        ScalarValue::String(v.as_str().unwrap().into())
                    }
                })
                .collect();
            match schema.tuple(&values).unwrap() {
                None => metrics.add_null_rows(1).unwrap(),
                Some(key) => *groups.entry(key).or_insert(0) += 1,
            }
        }
        for count in groups.values() {
            metrics.add_group(*count).unwrap();
        }
        let wanted: Vec<_> = case["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .collect();
        assert_eq!(
            vec![
                metrics.null_key_rows,
                metrics.duplicate_groups,
                metrics.duplicate_group_rows,
                metrics.violating_rows
            ],
            wanted,
            "{}",
            case["name"]
        );
        assert_eq!(metrics.passes(), metrics.violating_rows == 0);
    }
}
#[test]
fn null_nan_infinity_and_membership_do_not_inherit_engine_ordering() {
    let nan = float("NaN");
    assert_eq!(is_null(&nan), Truth::False);
    assert_eq!(is_nan(&nan).unwrap(), Truth::True);
    assert_eq!(is_finite(&nan).unwrap(), Truth::False);
    for op in [
        Comparison::Gt,
        Comparison::Gte,
        Comparison::Lt,
        Comparison::Lte,
        Comparison::Equals,
        Comparison::NotEquals,
    ] {
        assert_eq!(
            compare(op, &nan, &float("1.0")).unwrap(),
            Truth::from_bool(op == Comparison::NotEquals)
        );
        assert_eq!(
            compare(op, &float("1.0"), &nan).unwrap(),
            Truth::from_bool(op == Comparison::NotEquals)
        );
        assert_eq!(
            compare(op, &ScalarValue::Null, &float("1.0")).unwrap(),
            Truth::Unknown
        );
    }
    assert_eq!(is_finite(&float("Infinity")).unwrap(), Truth::False);
    assert_eq!(
        compare(Comparison::Equals, &float("Infinity"), &float("Infinity")).unwrap(),
        Truth::True
    );
    assert_eq!(
        compare(Comparison::Equals, &float("-0.0"), &float("0.0")).unwrap(),
        Truth::True
    );
    assert_eq!(
        is_in(&ScalarValue::Null, &[integer(1)]).unwrap(),
        Truth::False
    );
    assert_eq!(
        is_in(&ScalarValue::Null, &[integer(1), ScalarValue::Null]).unwrap(),
        Truth::True
    );
    assert_eq!(
        is_in(&nan, std::slice::from_ref(&nan)).unwrap(),
        Truth::False
    );
    assert_eq!(is_in(&integer(1), &[]).unwrap(), Truth::False);
    assert!(is_in(&integer(1), &[integer(1), ScalarValue::String("1".into())]).is_err());
    assert!(is_finite(&integer(1)).is_err());
}
#[test]
fn precision_and_supported_key_types_are_exact_even_for_empty_or_null_data() {
    let large = ScalarValue::Integer(
        IntegerValue::parse(IntegerType::U64, "18446744073709551615").unwrap(),
    );
    assert_eq!(
        compare(Comparison::Gt, &large, &integer(i64::MAX)).unwrap(),
        Truth::True
    );
    let decimal_type = DecimalType::new(38, 2).unwrap();
    let dec = |s| ScalarValue::Decimal(DecimalValue::parse(decimal_type, s).unwrap());
    assert_eq!(
        compare(
            Comparison::Lt,
            &dec("123456789012345678901234567890123456.77"),
            &dec("123456789012345678901234567890123456.78")
        )
        .unwrap(),
        Truth::True
    );
    let key = KeySchema::new(vec![LogicalType::Decimal(decimal_type)]).unwrap();
    assert_eq!(
        key.tuple(&[dec("-0.00")]).unwrap(),
        key.tuple(&[dec("0.00")]).unwrap()
    );
    let timestamp_type =
        TimestampType::new(TimeUnit::Nanoseconds, Some(Timezone::parse("UTC").unwrap()));
    let ts = |s| ScalarValue::Timestamp(TimestampValue::parse(timestamp_type.clone(), s).unwrap());
    let a = ts("2026-01-01T00:00:00.000000001Z");
    let b = ts("2026-01-01T00:00:00.000000002Z");
    assert_eq!(compare(Comparison::Lt, &a, &b).unwrap(), Truth::True);
    let key = KeySchema::new(vec![LogicalType::Timestamp(timestamp_type.clone())]).unwrap();
    assert_ne!(key.tuple(&[a]).unwrap(), key.tuple(&[b]).unwrap());
    let key = KeySchema::new(vec![LogicalType::Bool, LogicalType::Date]).unwrap();
    let date = ScalarValue::Date(DateValue::parse("2024-02-29").unwrap());
    assert_ne!(
        key.tuple(&[ScalarValue::Bool(true), date.clone()]).unwrap(),
        key.tuple(&[ScalarValue::Bool(false), date]).unwrap()
    );
    for typ in [
        LogicalType::Float(FloatWidth::F64),
        LogicalType::Binary,
        LogicalType::Struct(tf_domain::schema::Fields::new(vec![]).unwrap()),
    ] {
        assert!(KeySchema::new(vec![typ]).is_err());
    }
    let key = KeySchema::new(vec![
        LogicalType::String,
        LogicalType::Integer(IntegerType::I64),
    ])
    .unwrap();
    assert!(
        key.tuple(&[ScalarValue::Null, ScalarValue::String("wrong".into())])
            .is_err()
    );
    assert!(key.tuple(&[]).is_err());
    assert!(compare(Comparison::Equals, &integer(1), &float("1.0")).is_err());
}
#[test]
fn exact_metrics_reject_overflow_without_partial_mutation() {
    let mut key = PrimaryKeyMetrics::default();
    key.add_null_rows(u64::MAX).unwrap();
    let before = key;
    assert_eq!(key.add_group(2), Err(SemanticError::Overflow));
    assert_eq!(key, before);
    assert_eq!(key.add_group(0), Err(SemanticError::Shape));
    let mut row = RowMetrics {
        rows_evaluated: u64::MAX,
        ..Default::default()
    };
    let before = row;
    assert_eq!(
        row.observe(Truth::False, NullPolicy::Fail),
        Err(SemanticError::Overflow)
    );
    assert_eq!(row, before);
}
