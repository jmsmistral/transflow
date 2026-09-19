use crate::{Result, SCHEMA_VERSION, StoreError};
use sqlx::{Connection, Row, SqliteConnection};
const APP_ID: i64 = 0x5452464c;
const MIGRATIONS: [&str; 2] = [
    include_str!("../migrations/001_core.sql"),
    include_str!("../migrations/002_catalog_foreign.sql"),
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
    sqlx::raw_sql("PRAGMA application_id=1414678092; PRAGMA user_version=2;")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
