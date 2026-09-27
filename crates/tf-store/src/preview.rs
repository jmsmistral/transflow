//! Bounded projected physical pages; no whole-artifact decode or data replication.
use crate::{
    artifacts::{ArtifactError, ArtifactStore},
    normalization::{NormalizedSchema, cell},
};
use parquet::arrow::{ProjectionMask, arrow_reader::ParquetRecordBatchReaderBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{Read, Seek, SeekFrom},
    time::{Duration, Instant},
};
use tf_protocol::canonical::ContentDigest;
/// Position in immutable manifest order, not a semantic sort key.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Position {
    /// Manifest file ordinal.
    pub file: usize,
    /// Physical row group.
    pub group: usize,
    /// Row within that group.
    pub row: usize,
}
/// Bounded request already bound to a leased immutable version by the application.
pub struct Options<'a> {
    /// Projected top-level columns, in display order.
    pub columns: &'a [String],
    /// Maximum rows (1..=1000).
    pub rows: usize,
    /// Per-cell encoded bytes (up to 64 KiB).
    pub cell_bytes: usize,
    /// Encoded row budget, capped at 1.5 MiB to leave envelope/schema space.
    pub row_bytes: usize,
    /// Cooperative interactive deadline; None disables the timer.
    pub timeout: Option<Duration>,
}
/// A page with a physical continuation; values use the shared lossless wire encoding.
pub struct Page {
    /// Projected schema.
    pub schema: Value,
    /// Bounded typed cells; truncation is explicit and never a null value.
    pub rows: Vec<Vec<Value>>,
    /// Next exact physical position.
    pub next: Option<Position>,
}
/// Read at most requested rows, two MiB output, one projected row group at a time.
/// The caller checks renewable lease protection before each group and before returning.
pub fn read(
    store: &ArtifactStore,
    digest: ContentDigest,
    manifest: &Value,
    mut position: Position,
    options: Options<'_>,
    check: impl Fn() -> bool,
) -> Result<Page, ArtifactError> {
    if options.rows == 0
        || options.rows > 1000
        || options.columns.is_empty()
        || options.columns.len() > 128
        || options.cell_bytes == 0
        || options.cell_bytes > 65536
        || options.row_bytes == 0
        || options.row_bytes > 1536 * 1024
    {
        return Err(ArtifactError::Metadata);
    }
    let files = manifest["files"]
        .as_array()
        .ok_or(ArtifactError::Metadata)?;
    if position.file >= files.len() {
        return Err(ArtifactError::Metadata);
    }
    let mut rows = Vec::new();
    let mut bytes = 0;
    let mut schema = None;
    let started = Instant::now();
    let expired = || {
        options
            .timeout
            .is_some_and(|duration| started.elapsed() >= duration)
    };
    while position.file < files.len() {
        if !check() || expired() {
            return Err(ArtifactError::Integrity);
        }
        let mut file = store.preview_file(digest, manifest, position.file)?;
        let len = file.metadata()?.len();
        if len < 8 {
            return Err(ArtifactError::Integrity);
        }
        file.seek(SeekFrom::End(-8))?;
        let mut footer = [0; 8];
        file.read_exact(&mut footer)?;
        let metadata_len = u32::from_le_bytes([footer[0], footer[1], footer[2], footer[3]]) as u64;
        if &footer[4..] != b"PAR1" || metadata_len > 8 * 1024 * 1024 || metadata_len + 8 > len {
            return Err(ArtifactError::Metadata);
        }
        file.seek(SeekFrom::Start(0))?;
        let builder =
            ParquetRecordBatchReaderBuilder::try_new(file).map_err(|_| ArtifactError::Integrity)?;
        let full = NormalizedSchema::from_arrow(builder.schema())?;
        if full.value() != &manifest["logical_schema"] {
            return Err(ArtifactError::Integrity);
        }
        let mut indices = Vec::new();
        for name in options.columns {
            let index = builder
                .schema()
                .index_of(name)
                .map_err(|_| ArtifactError::Metadata)?;
            if indices.contains(&index) {
                return Err(ArtifactError::Metadata);
            }
            indices.push(index);
        }
        let fields = options
            .columns
            .iter()
            .map(|name| {
                full.value()["fields"]
                    .as_array()
                    .and_then(|fields| fields.iter().find(|f| f["name"] == *name))
                    .cloned()
                    .ok_or(ArtifactError::Metadata)
            })
            .collect::<Result<Vec<_>, _>>()?;
        schema = Some(json!({"format_version":1,"fields":fields}));
        let groups = builder.metadata().num_row_groups();
        if position.group >= groups {
            if position.group > groups || position.row != 0 {
                return Err(ArtifactError::Metadata);
            }
            position = Position {
                file: position.file + 1,
                ..Position::default()
            };
            continue;
        }
        let group = builder.metadata().row_group(position.group);
        let count = usize::try_from(group.num_rows()).map_err(|_| ArtifactError::Metadata)?;
        if position.row > count {
            return Err(ArtifactError::Metadata);
        }
        let projection = ProjectionMask::roots(builder.parquet_schema(), indices);
        let size = group
            .columns()
            .iter()
            .enumerate()
            .filter(|(i, _)| projection.leaf_included(*i))
            .try_fold(0u64, |sum, (_, c)| {
                u64::try_from(c.uncompressed_size())
                    .ok()
                    .and_then(|n| sum.checked_add(n))
            })
            .ok_or(ArtifactError::Metadata)?;
        // Arrow can decode a whole compressed page even for one output row. Bound the
        // selected row-group's declared uncompressed bytes, not merely the output LIMIT.
        if size > 64 * 1024 * 1024 {
            return Err(ArtifactError::Metadata);
        }
        let reader = builder
            .with_projection(projection)
            .with_row_groups(vec![position.group])
            .with_offset(position.row)
            .with_limit(options.rows - rows.len())
            .with_batch_size(128)
            .build()
            .map_err(|_| ArtifactError::Integrity)?;
        for batch in reader {
            let batch = batch.map_err(|_| ArtifactError::Integrity)?;
            for i in 0..batch.num_rows() {
                if !check() || expired() {
                    return Err(ArtifactError::Integrity);
                }
                let mut row = Vec::new();
                for name in options.columns {
                    let a = batch.column_by_name(name).ok_or(ArtifactError::Metadata)?;
                    let value = match cell(a.as_ref(), i) {
                        Ok(v) => {
                            if serde_json::to_vec(&v)
                                .map_err(|_| ArtifactError::Metadata)?
                                .len()
                                > options.cell_bytes
                            {
                                json!({"value":null,"truncated":true})
                            } else {
                                json!({"value":v,"truncated":false})
                            }
                        }
                        Err(e)
                            if matches!(
                                e.reason,
                                "Encoded cell exceeds 64 KiB limit"
                                    | "Cell exceeds 64 KiB payload limit"
                                    | "Cell exceeds nesting or element limit"
                                    | "Cell exceeds element limit"
                                    | "Value nesting exceeds limit"
                            ) =>
                        {
                            json!({"value":null,"truncated":true})
                        }
                        Err(e) => return Err(e.into()),
                    };
                    row.push(value);
                }
                let n = serde_json::to_vec(&row)
                    .map_err(|_| ArtifactError::Metadata)?
                    .len();
                if bytes + n > options.row_bytes {
                    if rows.is_empty() {
                        return Err(ArtifactError::Metadata);
                    }
                    return Ok(Page {
                        schema: schema.ok_or(ArtifactError::Metadata)?,
                        rows,
                        next: Some(position),
                    });
                }
                bytes += n;
                rows.push(row);
                position.row += 1;
            }
        }
        if position.row == count {
            position.group += 1;
            position.row = 0;
            if position.group == groups {
                position.file += 1;
                position.group = 0;
            }
        }
        if rows.len() == options.rows {
            break;
        }
    }
    if !check() || expired() {
        return Err(ArtifactError::Integrity);
    }
    Ok(Page {
        schema: schema.unwrap_or_else(|| json!({"format_version":1,"fields":[]})),
        rows,
        next: (position.file < files.len()).then_some(position),
    })
}
