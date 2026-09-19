//! Real Arrow/Parquet and exact typed-value boundary tests.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Synthetic assertions"
)]
use arrow_array::{types::Int64Type, *};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use parquet::arrow::{ArrowWriter, arrow_reader::ParquetRecordBatchReaderBuilder};
use serde_json::json;
use std::{
    fs::{self, File},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tf_store::normalization::{Adapter, NormalizedSchema, cell, inspect_parquet, validate_batch};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "tf-normalize-{}-{}.parquet",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
mod support {
    pub mod normalization_fixture;
}
use support::normalization_fixture::{batch, portable};
#[test]
fn parquet_roundtrip_preserves_exact_values_and_empty_schema() {
    let batch = portable();
    let schema = NormalizedSchema::from_arrow(batch.schema().as_ref()).unwrap();
    schema.require(Adapter::Parquet).unwrap();
    validate_batch(&batch, &schema).unwrap();
    for input in [batch.clone(), batch.slice(0, 0)] {
        let temp = Temp::new();
        let mut writer =
            ArrowWriter::try_new(File::create(&temp.0).unwrap(), input.schema(), None).unwrap();
        writer.write(&input).unwrap();
        writer.close().unwrap();
        let normalized = inspect_parquet(File::open(&temp.0).unwrap()).unwrap();
        assert_eq!(normalized.rows, input.num_rows() as u64);
        assert_eq!(normalized.schema, schema);
        let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(&temp.0).unwrap())
            .unwrap()
            .build()
            .unwrap();
        for actual in reader {
            let actual = actual.unwrap();
            for col in 0..actual.num_columns() {
                for row in 0..actual.num_rows() {
                    assert_eq!(
                        cell(actual.column(col).as_ref(), row).unwrap(),
                        cell(input.column(col).as_ref(), row).unwrap()
                    );
                }
            }
        }
    }
}
#[test]
fn exact_carriers_distinguish_precision_null_nan_and_negative_epoch() {
    let b = portable();
    assert_eq!(
        cell(b.column(1).as_ref(), 1).unwrap(),
        json!({"type":"u64","value":"18446744073709551615"})
    );
    assert_eq!(
        cell(b.column(2).as_ref(), 0).unwrap(),
        json!({"type":"f64","value":"NaN"})
    );
    assert_eq!(cell(b.column(2).as_ref(), 1).unwrap()["value"], "-0");
    assert_eq!(
        cell(b.column(2).as_ref(), 2).unwrap(),
        json!({"type":"null"})
    );
    assert_eq!(cell(b.column(3).as_ref(), 1).unwrap()["value"], "-123.4500");
    assert_eq!(
        cell(b.column(4).as_ref(), 0).unwrap()["value"],
        "1969-12-31T23:59:59.999999999"
    );
    assert_eq!(
        cell(b.column(5).as_ref(), 0).unwrap()["value"],
        "1969-12-31T23:59:59.999999999Z"
    );
    assert_eq!(
        cell(b.column(5).as_ref(), 0).unwrap()["timezone"],
        "Europe/London"
    );
    assert_eq!(cell(b.column(7).as_ref(), 0).unwrap()["value"], "AP8=");
    assert_eq!(
        cell(b.column(8).as_ref(), 0).unwrap()["value"],
        "0001-01-01"
    );
    assert_eq!(
        cell(b.column(8).as_ref(), 1).unwrap()["value"],
        "9999-12-31"
    );
    assert!(cell(b.column(0).as_ref(), 3).is_err());
}
#[test]
fn storage_representability_does_not_imply_engine_support() {
    for (ty, parquet, polars, duckdb) in [
        (
            DataType::Timestamp(TimeUnit::Nanosecond, None),
            true,
            true,
            true,
        ),
        (
            DataType::Timestamp(TimeUnit::Nanosecond, Some("UTC".into())),
            true,
            true,
            false,
        ),
        (
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            true,
            true,
            true,
        ),
        (
            DataType::Timestamp(TimeUnit::Microsecond, Some("Europe/London".into())),
            true,
            true,
            false,
        ),
        (
            DataType::Timestamp(TimeUnit::Second, None),
            false,
            false,
            false,
        ),
        (DataType::Decimal128(10, -2), false, false, false),
        (DataType::Decimal128(2, 4), false, false, false),
        (DataType::Decimal128(38, 4), true, true, true),
    ] {
        let schema =
            NormalizedSchema::from_arrow(&Schema::new(vec![Field::new("value", ty, true)]))
                .unwrap();
        for (adapter, ok) in [
            (Adapter::Parquet, parquet),
            (Adapter::Polars, polars),
            (Adapter::DuckDbValidation, duckdb),
            (Adapter::Pandas, false),
            (Adapter::DuckDbSql, false),
        ] {
            assert_eq!(schema.require(adapter).is_ok(), ok);
        }
    }
}
#[test]
fn physical_offsets_normalize_but_logical_changes_do_not() {
    let normal = |ty| {
        NormalizedSchema::from_arrow(&Schema::new(vec![Field::new("value", ty, true)])).unwrap()
    };
    assert_eq!(normal(DataType::Utf8), normal(DataType::LargeUtf8));
    assert_eq!(normal(DataType::Utf8), normal(DataType::Utf8View));
    assert_eq!(normal(DataType::Binary), normal(DataType::LargeBinary));
    assert_eq!(
        normal(DataType::List(Arc::new(Field::new(
            "item",
            DataType::Int64,
            true
        )))),
        normal(DataType::LargeList(Arc::new(Field::new(
            "element",
            DataType::Int64,
            true
        ))))
    );
    assert_ne!(normal(DataType::Int64), normal(DataType::UInt64));
    assert_ne!(
        normal(DataType::Decimal128(18, 2)),
        normal(DataType::Decimal128(18, 4))
    );
    assert_ne!(
        normal(DataType::Timestamp(TimeUnit::Microsecond, None)),
        normal(DataType::Timestamp(TimeUnit::Nanosecond, None))
    );
}
#[test]
fn duplicate_unsupported_extension_and_lossy_values_fail() {
    assert!(
        NormalizedSchema::from_arrow(&Schema::new(vec![
            Field::new("same", DataType::Int64, true),
            Field::new("same", DataType::Int32, true)
        ]))
        .is_err()
    );
    for ty in [
        DataType::Null,
        DataType::Date64,
        DataType::Duration(TimeUnit::Microsecond),
        DataType::Decimal256(40, 4),
        DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
    ] {
        assert!(
            NormalizedSchema::from_arrow(&Schema::new(vec![Field::new("bad", ty, true)])).is_err()
        );
    }
    let metadata: arrow_schema::Metadata =
        [("ARROW:extension:name".to_owned(), "object".to_owned())]
            .into_iter()
            .collect();
    assert!(
        NormalizedSchema::from_arrow(&Schema::new(vec![
            Field::new("object", DataType::Binary, true).with_metadata(metadata)
        ]))
        .is_err()
    );
    let b = batch(vec![("bad", Arc::new(Date32Array::from(vec![2932897])))]);
    let schema = NormalizedSchema::from_arrow(b.schema().as_ref()).unwrap();
    assert!(validate_batch(&b, &schema).is_err());
    let b = batch(vec![(
        "bad",
        Arc::new(
            Decimal128Array::from(vec![1234])
                .with_precision_and_scale(2, 0)
                .unwrap(),
        ),
    )]);
    let schema = NormalizedSchema::from_arrow(b.schema().as_ref()).unwrap();
    assert!(validate_batch(&b, &schema).is_err());
    assert!(cell(&StringArray::from(vec!["x".repeat(65536)]), 0).is_err());
    assert!(cell(&StringArray::from(vec!["\u{0001}".repeat(12000)]), 0).is_err());
}
#[test]
fn nested_fields_and_null_empty_list_remain_distinct() {
    let list = ListArray::from_iter_primitive::<Int64Type, _, _>(vec![
        None,
        Some(vec![]),
        Some(vec![None, Some(1)]),
    ]);
    assert_eq!(cell(&list, 0).unwrap(), json!({"type":"null"}));
    assert_eq!(cell(&list, 1).unwrap(), json!({"type":"list","values":[]}));
    assert_eq!(cell(&list, 2).unwrap()["values"][0], json!({"type":"null"}));
    let fields = vec![Arc::new(Field::new("id", DataType::Int64, true))];
    let structure = StructArray::new(
        fields.into(),
        vec![Arc::new(Int64Array::from(vec![Some(1), None]))],
        None,
    );
    assert_eq!(
        cell(&structure, 0).unwrap(),
        json!({"type":"struct","fields":[{"name":"id","value":{"type":"i64","value":"1"}}]})
    );
    let b = batch(vec![("nested", Arc::new(structure))]);
    let normalized = NormalizedSchema::from_arrow(b.schema().as_ref()).unwrap();
    validate_batch(&b, &normalized).unwrap();
}

#[test]
fn field_metadata_nullability_and_masked_struct_values_are_preserved() {
    let metadata: arrow_schema::Metadata = [("unit".to_owned(), "USD".to_owned())]
        .into_iter()
        .collect();
    let schema = NormalizedSchema::from_arrow(&Schema::new(vec![
        Field::new("amount", DataType::Int64, false).with_metadata(metadata),
    ]))
    .unwrap();
    assert_eq!(schema.value()["fields"][0]["metadata"]["unit"], "USD");
    assert_eq!(schema.value()["fields"][0]["nullable"], false);
    let other = NormalizedSchema::from_arrow(&Schema::new(vec![Field::new(
        "amount",
        DataType::Int64,
        true,
    )]))
    .unwrap();
    assert_ne!(schema.fingerprint(), other.fingerprint());
    let masks = BooleanArray::from(vec![None, Some(true)]);
    let structure = StructArray::new(
        vec![Arc::new(Field::new("required", DataType::Int64, false))].into(),
        vec![Arc::new(Int64Array::from(vec![None, Some(7)]))],
        masks.nulls().cloned(),
    );
    assert_eq!(cell(&structure, 0).unwrap(), json!({"type":"null"}));
    let b = batch(vec![("record", Arc::new(structure))]);
    validate_batch(
        &b,
        &NormalizedSchema::from_arrow(b.schema().as_ref()).unwrap(),
    )
    .unwrap();
    let nested = DataType::Struct(
        vec![Arc::new(Field::new(
            "time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("UTC".into())),
            true,
        ))]
        .into(),
    );
    let schema =
        NormalizedSchema::from_arrow(&Schema::new(vec![Field::new("record", nested, true)]))
            .unwrap();
    assert_eq!(
        schema.require(Adapter::DuckDbValidation).unwrap_err().path,
        "$.record.time"
    );
}
