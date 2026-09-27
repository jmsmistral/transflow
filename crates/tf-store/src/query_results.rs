//! Bounded Arrow IPC query results projected through the same lossless wire types as previews.
use crate::normalization::{NormalizedSchema, cell};
use arrow_ipc::reader::StreamReader;
use serde_json::Value;
use std::{
    fs::OpenOptions,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};
/// Invalid, oversized or unsupported query results. Cell values never enter diagnostics.
#[derive(Debug, thiserror::Error)]
#[error("Query result is unavailable, oversized or has unsupported types")]
pub struct Error;
/// Bounded in-memory result; the application owns expiry and pagination.
pub struct Page {
    /// Exact normalized schema.
    pub schema: Value,
    /// Lossless wire values in column order.
    pub rows: Vec<Value>,
    /// The JSON representation hit its independent byte ceiling.
    pub truncated: bool,
}
/// Read only a private, single-link regular file. Bound IPC and JSON separately.
pub fn read(path: &Path, max_rows: usize, max_bytes: usize) -> Result<Page, Error> {
    if max_rows > 1000 || max_bytes > 2 * 1024 * 1024 {
        return Err(Error);
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
        .map_err(|_| Error)?;
    let m = file.metadata().map_err(|_| Error)?;
    if !m.is_file() || m.nlink() != 1 || m.mode() & 0o077 != 0 || m.len() > max_bytes as u64 {
        return Err(Error);
    }
    let mut bytes = Vec::new();
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error)?;
    if bytes.len() > max_bytes {
        return Err(Error);
    }
    let reader = StreamReader::try_new(std::io::Cursor::new(bytes), None).map_err(|_| Error)?;
    if reader.schema().fields().len() > 128 {
        return Err(Error);
    }
    let schema = NormalizedSchema::from_arrow(&reader.schema())
        .map_err(|_| Error)?
        .value()
        .clone();
    let mut size = serde_json::to_vec(&schema).map_err(|_| Error)?.len() + 1024;
    if size > max_bytes {
        return Err(Error);
    }
    let mut page = Page {
        schema,
        rows: Vec::new(),
        truncated: false,
    };
    for batch in reader {
        let batch = batch.map_err(|_| Error)?;
        if page.rows.len() + batch.num_rows() > max_rows {
            return Err(Error);
        }
        for row in 0..batch.num_rows() {
            let values = batch
                .columns()
                .iter()
                .map(|a| cell(a.as_ref(), row).map_err(|_| Error))
                .collect::<Result<Vec<_>, _>>()?;
            size += serde_json::to_vec(&values).map_err(|_| Error)?.len() + 1;
            if size > max_bytes {
                page.truncated = true;
                return Ok(page);
            }
            page.rows.push(Value::Array(values));
        }
    }
    Ok(page)
}
