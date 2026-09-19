//! Constructor failures and domain invariants, without storage or engine mocks.
#![allow(clippy::unwrap_used, clippy::expect_used)] // Assertions on test setup/results only.

use std::collections::{BTreeMap, BTreeSet};
use tf_domain::{
    schema::{Field, FieldName, Fields, LogicalSchema, LogicalType},
    value::*,
    *,
};
const UUID: &str = "01234567-89ab-cdef-0123-456789abcdef";

#[test]
fn distinct_identity_roles_preserve_bytes_and_canonical_text() {
    let workspace: WorkspaceId = UUID.parse().unwrap();
    let dataset: DatasetId = UUID.parse().unwrap();
    let version: VersionId = UUID.parse().unwrap();
    let attempt: AttemptId = UUID.parse().unwrap();
    let branch: BranchId = UUID.parse().unwrap();
    let source: SourceSnapshotId = UUID.parse().unwrap();
    for text in [
        workspace.to_string(),
        dataset.to_string(),
        version.to_string(),
        attempt.to_string(),
        branch.to_string(),
        source.to_string(),
    ] {
        assert_eq!(text, UUID);
    }
    assert_eq!(DatasetId::from_bytes(*dataset.as_bytes()), dataset);
    assert_eq!(workspace.as_bytes(), dataset.as_bytes());
}
#[test]
fn malformed_uuids_fail_and_identity_members_have_locations() {
    for text in [
        "",
        "0123456789abcdef0123456789abcdef",
        "01234567-89AB-cdef-0123-456789abcdef",
        "01234567-89ab-cdef-0123-456789abcdeg",
        "01234567-89ab-cdef-0123-456789abcdef\n",
        "🦀🦀🦀🦀🦀🦀🦀🦀🦀",
    ] {
        assert_eq!(
            text.parse::<DatasetId>().unwrap_err().kind(),
            ErrorKind::InvalidUuid
        );
    }
    assert_eq!(
        DatasetKey::from_text("bad", UUID).unwrap_err().location(),
        "/workspace_id"
    );
    assert_eq!(
        DatasetKey::from_text(UUID, "bad").unwrap_err().location(),
        "/dataset_id"
    );
}
#[test]
fn names_and_renames_do_not_define_identity_or_ownership() {
    let workspace = WorkspaceId::from_bytes([1; 16]);
    let foreign = WorkspaceId::from_bytes([2; 16]);
    let dataset = DatasetId::from_bytes([3; 16]);
    let local = DatasetKey::new(workspace, dataset);
    let remote = DatasetKey::new(foreign, dataset);
    assert_ne!(local, remote);
    assert_eq!(local.scope(workspace), DatasetScope::Local);
    assert_eq!(remote.scope(workspace), DatasetScope::Foreign);
    assert_eq!(local.scope(foreign), DatasetScope::Foreign);
    let old = DatasetPath::parse_for_scope("raw/orders", DatasetScope::Local).unwrap();
    let renamed = DatasetPath::parse_for_scope("curated/purchases", DatasetScope::Local).unwrap();
    assert_ne!(old, renamed);
    let previous_binding = (local, old);
    let new_binding = (local, renamed);
    assert_eq!(previous_binding.0, new_binding.0);
    assert_eq!(BTreeSet::from([local, remote]).len(), 2);
}
#[test]
fn dataset_paths_reject_escapes_and_keywords_at_the_bad_segment() {
    for text in [
        "raw/../orders",
        "raw/class",
        "raw/_private",
        "raw/Orders",
        "raw/orders/",
        "raw//orders",
        "raw/a-b",
        "raw/a:b",
        "raw/🦀",
        "raw/a\\b",
    ] {
        let error = text.parse::<DatasetPath>().unwrap_err();
        assert!(
            error.location().starts_with("/segments/"),
            "{text}: {error}"
        );
    }
    let keyword = "raw/class".parse::<DatasetPath>().unwrap_err();
    assert_eq!(keyword.kind(), ErrorKind::ReservedKeyword);
    assert_eq!(keyword.location(), "/segments/1");
    assert!(keyword.to_string().contains("Python keyword"));
    for text in ["", "/raw", "../raw", "raw/\n"] {
        assert!(text.parse::<DatasetPath>().is_err());
    }
    for text in [
        "raw/orders",
        "raw/orders/daily",
        "match/case",
        "raw/order_2",
    ] {
        assert_eq!(text.parse::<DatasetPath>().unwrap().as_str(), text);
    }
}
#[test]
fn external_namespace_requires_foreign_scope_and_an_alias() {
    assert_eq!(
        DatasetPath::parse_for_scope("external/provider/orders", DatasetScope::Local)
            .unwrap_err()
            .kind(),
        ErrorKind::ReservedNamespace
    );
    for text in ["external", "raw/orders"] {
        assert_eq!(
            DatasetPath::parse_for_scope(text, DatasetScope::Foreign)
                .unwrap_err()
                .kind(),
            ErrorKind::MissingExternalAlias
        );
    }
    assert!(
        DatasetPath::parse_for_scope("external/provider/orders", DatasetScope::Foreign).is_ok()
    );
}
#[test]
fn branch_names_are_opaque_and_selectors_do_not_imply_fallback() {
    for text in [
        "master",
        "feature/rewrite",
        "Team/Δ",
        "../metadata",
        "data branch",
    ] {
        let branch: BranchName = text.parse().unwrap();
        assert_eq!(branch.as_str(), text);
        for permission in [FallbackPermission::Allowed, FallbackPermission::Prohibited] {
            let binding = (BranchSelector::Named(branch.clone()), permission);
            assert_eq!(binding.1, permission);
        }
    }
    assert_ne!(BranchSelector::Omitted, BranchSelector::Current);
    assert_ne!(
        "master".parse::<BranchName>().unwrap(),
        "Master".parse().unwrap()
    );
    for text in ["", "feature\nnext", "a\0b", "a\u{0085}b"] {
        assert!(text.parse::<BranchName>().is_err());
    }
}
#[test]
fn integer_boundaries_round_trip_without_floating_point() {
    for (kind, low, high) in [
        (IntegerType::I8, -128, 127),
        (IntegerType::I16, -32768, 32767),
        (IntegerType::I32, i32::MIN as i128, i32::MAX as i128),
        (IntegerType::I64, i64::MIN as i128, i64::MAX as i128),
        (IntegerType::U8, 0, 255),
        (IntegerType::U16, 0, 65535),
        (IntegerType::U32, 0, u32::MAX as i128),
        (IntegerType::U64, 0, u64::MAX as i128),
    ] {
        for n in [low, high] {
            let value = IntegerValue::parse(kind, &n.to_string()).unwrap();
            assert_eq!(value.as_i128(), n);
            assert_eq!(value.kind(), kind);
            assert_eq!(value.to_text(), n.to_string());
        }
        for n in [low - 1, high + 1] {
            assert_eq!(
                IntegerValue::parse(kind, &n.to_string())
                    .unwrap_err()
                    .kind(),
                ErrorKind::OutOfRange
            );
        }
    }
    for raw in ["01", "-0", "+1", "1.0", "1e3", "1\n", "١", ""] {
        assert!(IntegerValue::parse(IntegerType::I64, raw).is_err());
    }
}
#[test]
fn float_specials_signed_zero_and_subnormals_survive() {
    for width in [FloatWidth::F32, FloatWidth::F64] {
        for text in ["-0.0", "0", "NaN", "Infinity", "-Infinity", "2.5e-12"] {
            let value = FloatValue::parse(width, text).unwrap();
            assert_eq!(value.as_str(), text);
            assert_eq!(value.width(), width);
        }
        assert!(
            FloatValue::parse(width, "-0.0")
                .unwrap()
                .as_f64()
                .is_sign_negative()
        );
        assert!(FloatValue::parse(width, "NaN").unwrap().as_f64().is_nan());
        for text in ["01.0", "nan", "+1", "1e999", "1e-999"] {
            assert!(FloatValue::parse(width, text).is_err());
        }
    }
    assert!(FloatValue::parse(FloatWidth::F32, "1e-46").is_err());
    assert!(FloatValue::parse(FloatWidth::F32, "3.5e38").is_err());
    assert!(
        FloatValue::parse(FloatWidth::F32, "1.401298464324817e-45")
            .unwrap()
            .as_f64()
            > 0.0
    );
    assert_eq!(
        FloatValue::parse(FloatWidth::F64, "5e-324")
            .unwrap()
            .as_f64()
            .to_bits(),
        1
    );
    assert_ne!(
        ScalarValue::Null,
        ScalarValue::Float(FloatValue::parse(FloatWidth::F64, "NaN").unwrap())
    );
}
#[test]
fn decimal_coefficients_scale_and_negative_zero_are_exact() {
    for (text, precision, scale, coefficient) in [
        (
            "99999999999999999999999999999999999999",
            38,
            0,
            99999999999999999999999999999999999999i128,
        ),
        ("-123.4500", 18, 4, -1234500),
        ("0.0001", 1, 4, 1),
        ("12300", 3, -2, 123),
        ("-0.00", 1, 2, 0),
        ("0", 1, -38, 0),
    ] {
        let kind = DecimalType::new(precision, scale).unwrap();
        let value = DecimalValue::parse(kind, text).unwrap();
        assert_eq!(value.coefficient(), coefficient);
        assert_eq!(value.as_str(), text);
        assert_eq!(value.kind(), kind);
    }
    for (text, p, s) in [
        ("12301", 3, -2),
        ("123.45", 4, 2),
        ("1.0", 2, 2),
        ("1e3", 4, 0),
        ("01.00", 4, 2),
        ("1.", 1, 0),
    ] {
        let error = DecimalValue::parse(DecimalType::new(p, s).unwrap(), text).unwrap_err();
        assert_eq!(error.location(), "/value");
    }
    for (p, s) in [(0, 0), (39, 0), (1, 39), (1, -39)] {
        assert!(DecimalType::new(p, s).is_err());
    }
}
#[test]
fn dates_validate_gregorian_boundaries() {
    for text in ["0001-01-01", "9999-12-31", "2000-02-29", "2024-02-29"] {
        assert_eq!(DateValue::parse(text).unwrap().as_str(), text);
    }
    for text in [
        "0000-01-01",
        "1900-02-29",
        "2023-02-29",
        "2026-04-31",
        "2026-00-01",
        "2026-1-01",
        "🦀",
    ] {
        assert!(DateValue::parse(text).is_err());
    }
}
#[test]
fn timestamps_preserve_unit_zone_and_naive_semantics() {
    for (unit, fraction) in [
        (TimeUnit::Seconds, ""),
        (TimeUnit::Milliseconds, ".123"),
        (TimeUnit::Microseconds, ".123456"),
        (TimeUnit::Nanoseconds, ".123456789"),
    ] {
        for zone in [None, Some("UTC"), Some("Asia/Tokyo")] {
            let kind = TimestampType::new(unit, zone.map(|s| Timezone::parse(s).unwrap()));
            let text = format!(
                "2026-09-19T12:34:56{fraction}{}",
                if zone.is_some() { "Z" } else { "" }
            );
            let value = TimestampValue::parse(kind.clone(), &text).unwrap();
            assert_eq!(value.as_str(), text);
            assert_eq!(value.kind(), &kind);
            assert_eq!(value.kind().timezone().map(Timezone::as_str), zone);
        }
    }
    let naive = TimestampType::new(TimeUnit::Nanoseconds, None);
    for text in [
        "2026-09-19T12:34:56.123456",
        "2026-09-19T12:34:56.123456789Z",
        "2026-09-19T12:34:60.123456789",
        "2026-09-19T24:00:00.123456789",
        "2026-02-30T00:00:00.123456789",
    ] {
        assert!(TimestampValue::parse(naive.clone(), text).is_err());
    }
    let aware = TimestampType::new(TimeUnit::Seconds, Some(Timezone::parse("UTC").unwrap()));
    for text in ["2026-09-19T12:34:56", "2026-09-19T12:34:56+00:00"] {
        assert!(TimestampValue::parse(aware.clone(), text).is_err());
    }
    for zone in ["", "not_zone", "A//B", "Europe/../London"] {
        assert!(Timezone::parse(zone).is_err());
    }
}
fn field(name: &str) -> Field {
    Field::new(
        FieldName::new(name).unwrap(),
        LogicalType::Integer(IntegerType::I64),
        true,
        None,
    )
}
#[test]
fn schemas_preserve_order_nullability_and_optional_metadata() {
    let nested = LogicalType::Struct(Fields::new(vec![field("z"), field("a")]).unwrap());
    let metadata = Some(BTreeMap::from([(
        "unit".to_owned(),
        "synthetic".to_owned(),
    )]));
    let parent = Field::new(
        FieldName::new("nested").unwrap(),
        LogicalType::List(Box::new(Field::new(
            FieldName::new("element").unwrap(),
            nested,
            false,
            metadata.clone(),
        ))),
        true,
        None,
    );
    let schema = LogicalSchema::new(vec![parent.clone(), field("second")]).unwrap();
    assert_eq!(schema.fields(), &[parent, field("second")]);
    assert!(LogicalSchema::new(vec![]).is_ok());
    let annotated = Field::new(
        FieldName::new("annotated").unwrap(),
        LogicalType::String,
        false,
        metadata.clone(),
    );
    assert_eq!(annotated.metadata(), metadata.as_ref());
    assert!(!annotated.nullable());
    assert_ne!(
        field("a"),
        Field::new(
            FieldName::new("a").unwrap(),
            LogicalType::Integer(IntegerType::I64),
            true,
            Some(BTreeMap::new())
        )
    );
}
#[test]
fn duplicate_schema_and_value_fields_report_the_second_name() {
    let error = LogicalSchema::new(vec![field("x"), field("x")]).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::DuplicateName);
    assert_eq!(error.location(), "/fields/1/name");
    assert_eq!(
        Fields::new(vec![field("x"), field("x")])
            .unwrap_err()
            .location(),
        "/fields/1/name"
    );
    let name = FieldName::new("x").unwrap();
    let null = Value::Scalar(ScalarValue::Null);
    let error = StructValue::new(vec![(name.clone(), null.clone()), (name, null)]).unwrap_err();
    assert_eq!(error.location(), "/fields/1/name");
    assert_eq!(
        error.at("inner/field~").location(),
        "/inner~1field~0/fields/1/name"
    );
}
#[test]
fn field_name_limits_count_unicode_scalars_without_normalization() {
    assert!(FieldName::new("🦀".repeat(256)).is_ok());
    assert!(FieldName::new("🦀".repeat(257)).is_err());
    for text in ["", "a\n", "a\x7f"] {
        assert!(FieldName::new(text).is_err());
    }
    assert_ne!(
        FieldName::new("é").unwrap(),
        FieldName::new("e\u{0301}").unwrap()
    );
}
#[test]
fn nested_values_preserve_binary_and_field_order() {
    let values = vec![
        (
            FieldName::new("z").unwrap(),
            Value::Scalar(ScalarValue::Binary(vec![0, 255, 128])),
        ),
        (
            FieldName::new("a").unwrap(),
            Value::List(vec![Value::Scalar(ScalarValue::Null)]),
        ),
    ];
    let structured = StructValue::new(values.clone()).unwrap();
    assert_eq!(structured.fields(), values.as_slice());
}
