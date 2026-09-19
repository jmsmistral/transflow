//! Repository-owned precision fixture, shared by tests and the explicit generator.
#![allow(clippy::unwrap_used, reason = "Synthetic fixture construction")]
use arrow_array::{types::Int64Type, *};
use arrow_schema::{Field, Schema};
use std::sync::Arc;
pub fn batch(arrays: Vec<(&str, ArrayRef)>) -> RecordBatch {
    let fields = arrays
        .iter()
        .map(|(n, a)| Field::new(*n, a.data_type().clone(), true))
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        arrays.into_iter().map(|(_, a)| a).collect(),
    )
    .unwrap()
}
pub fn portable() -> RecordBatch {
    batch(vec![
        (
            "i64",
            Arc::new(Int64Array::from(vec![Some(i64::MIN), Some(i64::MAX), None])),
        ),
        (
            "u64",
            Arc::new(UInt64Array::from(vec![Some(0), Some(u64::MAX), None])),
        ),
        (
            "float",
            Arc::new(Float64Array::from(vec![Some(f64::NAN), Some(-0.0), None])),
        ),
        (
            "decimal",
            Arc::new(
                Decimal128Array::from(vec![
                    Some(99999999999999999999999999999999999999i128),
                    Some(-1234500),
                    None,
                ])
                .with_precision_and_scale(38, 4)
                .unwrap(),
            ),
        ),
        (
            "naive_ns",
            Arc::new(TimestampNanosecondArray::from(vec![
                Some(-1),
                Some(1750000000123456789),
                None,
            ])),
        ),
        (
            "aware_ns",
            Arc::new(
                TimestampNanosecondArray::from(vec![Some(-1), Some(1750000000123456789), None])
                    .with_timezone("Europe/London"),
            ),
        ),
        (
            "text",
            Arc::new(LargeStringArray::from(vec![Some("é"), Some(""), None])),
        ),
        (
            "bytes",
            Arc::new(BinaryArray::from(vec![
                Some(&[0, 255][..]),
                Some(&[][..]),
                None,
            ])),
        ),
        (
            "date",
            Arc::new(Date32Array::from(vec![Some(-719162), Some(2932896), None])),
        ),
        (
            "list",
            Arc::new(ListArray::from_iter_primitive::<Int64Type, _, _>(vec![
                Some(vec![Some(7), None]),
                Some(vec![]),
                None,
            ])),
        ),
        (
            "bool",
            Arc::new(BooleanArray::from(vec![Some(true), Some(false), None])),
        ),
        (
            "i8",
            Arc::new(Int8Array::from(vec![Some(i8::MIN), Some(i8::MAX), None])),
        ),
        (
            "i16",
            Arc::new(Int16Array::from(vec![Some(i16::MIN), Some(i16::MAX), None])),
        ),
        (
            "i32",
            Arc::new(Int32Array::from(vec![Some(i32::MIN), Some(i32::MAX), None])),
        ),
        (
            "u8",
            Arc::new(UInt8Array::from(vec![Some(u8::MIN), Some(u8::MAX), None])),
        ),
        (
            "u16",
            Arc::new(UInt16Array::from(vec![
                Some(u16::MIN),
                Some(u16::MAX),
                None,
            ])),
        ),
        (
            "u32",
            Arc::new(UInt32Array::from(vec![
                Some(u32::MIN),
                Some(u32::MAX),
                None,
            ])),
        ),
        (
            "f32",
            Arc::new(Float32Array::from(vec![
                Some(f32::INFINITY),
                Some(f32::NEG_INFINITY),
                Some(f32::NAN),
            ])),
        ),
        (
            "naive_ms",
            Arc::new(TimestampMillisecondArray::from(vec![
                Some(-1),
                Some(1750000000123),
                None,
            ])),
        ),
        (
            "aware_us",
            Arc::new(
                TimestampMicrosecondArray::from(vec![Some(-1), Some(1750000000123456), None])
                    .with_timezone("UTC"),
            ),
        ),
        (
            "record",
            Arc::new(StructArray::new(
                vec![Arc::new(Field::new(
                    "id",
                    arrow_schema::DataType::UInt64,
                    true,
                ))]
                .into(),
                vec![Arc::new(UInt64Array::from(vec![
                    Some(u64::MAX),
                    Some(0),
                    None,
                ]))],
                None,
            )),
        ),
    ])
}
