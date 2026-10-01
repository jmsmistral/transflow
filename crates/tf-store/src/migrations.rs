use crate::{Result, SCHEMA_VERSION, StoreError};
use sqlx::{Connection, Row, SqliteConnection};
const APP_ID: i64 = 0x5452464c;
const MIGRATIONS: [&str; 13] = [
    include_str!("../migrations/001_core.sql"),
    include_str!("../migrations/002_catalog_foreign.sql"),
    include_str!("../migrations/003_publication.sql"),
    include_str!("../migrations/004_retention.sql"),
    include_str!("../migrations/005_replay.sql"),
    include_str!("../migrations/006_cache.sql"),
    include_str!("../migrations/007_freshness.sql"),
    include_str!("../migrations/008_check_evidence.sql"),
    include_str!("../migrations/009_foreign_inputs.sql"),
    include_str!("../migrations/010_direct_external_reads.sql"),
    include_str!("../migrations/011_api_operations.sql"),
    include_str!("../migrations/012_events.sql"),
    include_str!("../migrations/013_schedule_snapshots.sql"),
];
pub(crate) async fn preflight(db: &mut SqliteConnection) -> Result<()> {
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut *db)
        .await?;
    if version > SCHEMA_VERSION {
        return Err(StoreError::NewerSchema);
    }
    let app: i64 = sqlx::query_scalar("PRAGMA application_id")
        .fetch_one(&mut *db)
        .await?;
    let tables: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'")
            .fetch_one(&mut *db)
            .await?;
    if (app != APP_ID && !(app == 0 && version == 0 && tables == 0)) || version < 0 {
        return Err(StoreError::Migration);
    }
    Ok(())
}
pub(crate) async fn apply(db: &mut SqliteConnection) -> Result<()> {
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    preflight(&mut tx).await?;
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut *tx)
        .await?;
    sqlx::raw_sql("CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY, checksum TEXT NOT NULL CHECK(length(checksum)=64)) STRICT;").execute(&mut *tx).await?;
    let rows = sqlx::query("SELECT version,checksum FROM schema_migrations ORDER BY version")
        .fetch_all(&mut *tx)
        .await?;
    if rows.len() != version as usize {
        return Err(StoreError::Migration);
    }
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let n = (i + 1) as i64;
        let checksum = tf_protocol::canonical::file_digest(&mut sql.as_bytes())
            .map_err(|_| StoreError::Migration)?
            .hex();
        if n <= version {
            let row = rows.get(i).ok_or(StoreError::Migration)?;
            if row.try_get::<i64, _>(0)? != n || row.try_get::<String, _>(1)? != checksum {
                return Err(StoreError::Migration);
            }
        } else {
            sqlx::raw_sql(*sql).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO schema_migrations VALUES(?,?)")
                .bind(n)
                .bind(checksum)
                .execute(&mut *tx)
                .await?;
        }
    }
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "PRAGMA application_id={APP_ID}; PRAGMA user_version={SCHEMA_VERSION};"
    )))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Preserve the pre-release placeholder history before intentionally replacing it.
/// The caller holds the runtime owner; VACUUM INTO includes committed WAL content.
pub(crate) async fn backup_schedule_placeholders(
    db: &mut SqliteConnection,
    path: &std::path::Path,
) -> Result<()> {
    use std::{
        fs::{self, OpenOptions},
        os::unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    };
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut *db)
        .await?;
    if !(1..13).contains(&version) {
        return Ok(());
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM schedules")
        .fetch_one(&mut *db)
        .await?;
    if count == 0 {
        return Ok(());
    }
    let nonce: String = sqlx::query_scalar("SELECT lower(hex(randomblob(16)))")
        .fetch_one(&mut *db)
        .await?;
    let parent = path.parent().ok_or(StoreError::Filesystem)?;
    let backup = parent.join(format!("schedule-schema-{version}-backup-{nonce}.sqlite"));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&backup)?;
    let before = file.metadata()?;
    let name =
        std::str::from_utf8(backup.as_os_str().as_bytes()).map_err(|_| StoreError::Filesystem)?;
    sqlx::query("VACUUM INTO ?")
        .bind(name)
        .execute(&mut *db)
        .await?;
    let after = fs::symlink_metadata(&backup)?;
    if !after.is_file() || before.dev() != after.dev() || before.ino() != after.ino() {
        return Err(StoreError::Filesystem);
    }
    file.sync_all()?;
    fs::File::open(parent)?.sync_all()?;
    // The backup stays private and is never silently overwritten or automatically removed.
    Ok(())
}
