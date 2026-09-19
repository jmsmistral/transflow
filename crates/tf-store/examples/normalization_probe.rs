//! Synthetic cross-engine fixture generation and application normalization verification.
use parquet::arrow::{ArrowWriter, arrow_reader::ParquetRecordBatchReaderBuilder};
use serde_json::json;
use std::{
    error::Error,
    fs::{self, File},
    path::PathBuf,
};
use tf_store::normalization::{cell, inspect_parquet};
#[path = "../tests/support/normalization_fixture.rs"]
mod fixture;
fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let mode = args.next().ok_or("Expected generate or inspect")?;
    let path = PathBuf::from(args.next().ok_or("Expected explicit file/directory")?);
    if mode == "generate" {
        fs::create_dir(&path)?;
        let batch = fixture::portable();
        let mut writer = ArrowWriter::try_new(
            File::create(path.join("precision.parquet"))?,
            batch.schema(),
            None,
        )?;
        writer.write(&batch)?;
        writer.close()?;
    } else if mode != "inspect" {
        return Err("Expected generate or inspect".into());
    }
    let path = if mode == "generate" {
        path.join("precision.parquet")
    } else {
        path
    };
    let normalized = inspect_parquet(File::open(&path)?)?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(&path)?)?.build()?;
    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch?;
        for row in 0..batch.num_rows() {
            if rows.len() >= 100 {
                return Err("Probe only supports synthetic fixtures up to 100 rows".into());
            }
            rows.push(
                batch
                    .columns()
                    .iter()
                    .map(|a| cell(a.as_ref(), row))
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
    }
    println!(
        "{}",
        json!({"schema":normalized.schema.value(),"schema_fingerprint":normalized.schema.fingerprint(),"rows":rows})
    );
    Ok(())
}
