//! Explicit generator for repository-owned synthetic Parquet test inputs.
use arrow_array::{Int64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use std::{
    error::Error,
    fs::{self, File},
    path::PathBuf,
    sync::Arc,
};
fn main() -> Result<(), Box<dyn Error>> {
    let destination = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("Pass an explicit fixture output directory")?,
    );
    fs::create_dir_all(&destination)?;
    for (name, values) in [
        ("empty.parquet", vec![]),
        ("two-rows.parquet", vec![1i64, 2]),
    ] {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(Int64Array::from(values))])?;
        let mut writer = ArrowWriter::try_new(File::create(destination.join(name))?, schema, None)?;
        writer.write(&batch)?;
        writer.close()?;
    }
    Ok(())
}
