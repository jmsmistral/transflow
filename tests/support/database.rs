//! A real bundled SQLite fixture with an intentionally tiny, non-product schema.
use sqlx::{
    Connection, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
};
use std::{error::Error, path::Path};

pub async fn connect(path: &Path) -> Result<SqliteConnection, sqlx::Error> {
    SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full),
    )
    .await
}

pub async fn prepare(path: &Path) -> Result<(), Box<dyn Error>> {
    let mut db = connect(path).await?;
    let version: String = sqlx::query_scalar("SELECT sqlite_version()")
        .fetch_one(&mut db)
        .await?;
    assert_eq!(
        version, "3.51.3",
        "Fixture must use the qualified bundled SQLite"
    );
    sqlx::raw_sql(include_str!("../../fixtures/harness.sql"))
        .execute(&mut db)
        .await?;
    db.close().await?;
    Ok(())
}

pub async fn head(path: &Path) -> Result<String, sqlx::Error> {
    let mut db = connect(path).await?;
    let value = sqlx::query_scalar("SELECT artifact FROM fixture_head WHERE id = 1")
        .fetch_one(&mut db)
        .await?;
    db.close().await?;
    Ok(value)
}
