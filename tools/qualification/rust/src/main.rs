//! Native dependency smoke tests. This executable is not Transflow.

use std::{error::Error, fs::File, path::PathBuf, sync::Arc};

use arrow_array::{Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use axum::{Router, body::Body, http::Request, routing::get};
use chrono::TimeZone;
use clap::Parser;
use fs4::FileExt;
use parquet::arrow::{ArrowWriter, arrow_reader::ParquetRecordBatchReaderBuilder};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use tower::ServiceExt;

#[derive(Parser)]
struct Arguments {
    /// Empty probe directory; never point this at a user workspace or catalogue.
    #[arg(long)]
    output_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
enum QualificationError {
    #[error("{0}")]
    Check(String),
}

fn require(condition: bool, message: &str) -> Result<(), QualificationError> {
    if condition {
        Ok(())
    } else {
        Err(QualificationError::Check(message.to_owned()))
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct Report {
    sqlite_version: String,
    sqlite_source_id: String,
    sqlite_journal_mode: String,
    parquet_rows: usize,
    checks: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    require(
        format!("{:x}", Sha256::digest(b"abc"))
            == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        "SHA-256 known-answer vector failed",
    )?;
    let registry: toml::Table = toml::from_str("format_version = 1")?;
    require(
        registry
            .get("format_version")
            .and_then(toml::Value::as_integer)
            == Some(1),
        "TOML registry parse failed",
    )?;
    require(
        toml::from_str::<toml::Table>("format_version = 1\nformat_version = 2").is_err(),
        "TOML duplicate keys must fail",
    )?;
    let args = Arguments::parse();
    std::fs::create_dir_all(&args.output_dir)?;
    require(
        std::fs::read_dir(&args.output_dir)?.next().is_none(),
        "Probe output directory must be empty",
    )?;
    rustix::fs::statfs(&args.output_dir)?;
    let options = SqliteConnectOptions::new()
        .filename(args.output_dir.join("probe.sqlite"))
        .create_if_missing(true)
        .foreign_keys(true);
    let mut db = SqliteConnection::connect_with(&options).await?;
    let sqlite_version: String = sqlx::query_scalar("SELECT sqlite_version()")
        .fetch_one(&mut db)
        .await?;
    let components: Vec<u32> = sqlite_version
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    require(
        components.len() == 3 && components.as_slice() >= [3, 51, 3].as_slice(),
        "Linked SQLite must include the WAL-reset fix (>= 3.51.3)",
    )?;
    let sqlite_source_id = sqlx::query_scalar("SELECT sqlite_source_id()")
        .fetch_one(&mut db)
        .await?;
    let journal: String = sqlx::query_scalar("PRAGMA journal_mode=WAL")
        .fetch_one(&mut db)
        .await?;
    require(journal == "wal", "WAL mode was not enabled")?;
    sqlx::query("PRAGMA synchronous=FULL")
        .execute(&mut db)
        .await?;
    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut db)
        .await?;
    let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
        .fetch_one(&mut db)
        .await?;
    require(
        foreign_keys == 1 && synchronous == 2,
        "SQLite connection pragmas did not take effect",
    )?;
    sqlx::query("CREATE TABLE probe (id INTEGER PRIMARY KEY)")
        .execute(&mut db)
        .await?;
    let mut tx = db.begin().await?;
    sqlx::query("INSERT INTO probe VALUES (1)")
        .execute(&mut *tx)
        .await?;
    tx.rollback().await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM probe")
        .fetch_one(&mut db)
        .await?;
    require(count == 0, "Rolled-back row remained visible")?;
    db.close().await?;

    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, true),
        Field::new("label", DataType::Utf8, true),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from(vec![
                Some(9_007_199_254_740_993),
                Some(2),
                None,
            ])),
            Arc::new(StringArray::from(vec![Some("alpha"), None, Some("gamma")])),
        ],
    )?;
    let parquet_path = args.output_dir.join("rust.parquet");
    let mut writer = ArrowWriter::try_new(File::create(&parquet_path)?, schema, None)?;
    writer.write(&batch)?;
    writer.close()?;
    let batches = ParquetRecordBatchReaderBuilder::try_new(File::open(&parquet_path)?)?
        .build()?
        .collect::<Result<Vec<_>, _>>()?;
    require(
        batches == vec![batch],
        "Arrow/Parquet round trip changed values or nulls",
    )?;

    let lock = File::create(args.output_dir.join("probe.lock"))?;
    FileExt::try_lock(&lock)?;
    let contender = File::open(args.output_dir.join("probe.lock"))?;
    require(
        matches!(
            FileExt::try_lock(&contender),
            Err(fs4::TryLockError::WouldBlock)
        ),
        "Independent handle acquired an occupied lock",
    )?;
    FileExt::unlock(&lock)?;
    FileExt::try_lock(&contender)?;
    FileExt::unlock(&contender)?;

    let app = Router::new().route("/probe", get(|| async { "qualified" }));
    let response = app
        .oneshot(Request::builder().uri("/probe").body(Body::empty())?)
        .await?;
    require(
        response.status() == 200,
        "Axum/Tower request did not succeed",
    )?;
    require(
        chrono_tz::America::New_York
            .with_ymd_and_hms(2026, 3, 8, 2, 30, 0)
            .single()
            .is_none(),
        "Timezone library did not identify the DST gap",
    )?;
    let report = Report {
        sqlite_version,
        sqlite_source_id,
        sqlite_journal_mode: journal,
        parquet_rows: 3,
        checks: [
            "sqlite_version",
            "wal_full_foreign_keys",
            "rollback",
            "parquet_roundtrip",
            "advisory_lock",
            "http_router",
            "dst_gap",
            "json_schema",
            "sha256",
        ]
        .map(str::to_owned)
        .to_vec(),
    };
    let json = serde_json::to_string_pretty(&report)?;
    let _: Report = serde_json::from_str(&json)?;
    let schema = schemars::schema_for!(Report);
    require(
        schema.as_value().get("properties").is_some(),
        "JSON schema has no properties",
    )?;
    tracing::info!("Native dependency probe completed");
    println!("{json}");
    Ok(())
}
