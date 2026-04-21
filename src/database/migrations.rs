use sqlx::migrate::{Migrate, Migrator};
use sqlx::SqlitePool;
use tracing::info;
use crate::error::Result;

pub async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    run_migrator(pool, sqlx::migrate!("./src/database/migrations"), "database").await
}

pub async fn run_archive_migrations(pool: &SqlitePool) -> Result<()> {
    run_migrator(
        pool,
        sqlx::migrate!("./src/database/migrations/archive"),
        "archive database",
    )
    .await
}

async fn run_migrator(pool: &SqlitePool, migrator: Migrator, label: &str) -> Result<()> {
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| crate::error::WrightError::DatabaseError(format!("failed to acquire migration connection: {}", e)))?;

    conn.ensure_migrations_table()
        .await
        .map_err(|e| crate::error::WrightError::DatabaseError(format!("failed to ensure migrations table: {}", e)))?;

    let applied = conn
        .list_applied_migrations()
        .await
        .map_err(|e| crate::error::WrightError::DatabaseError(format!("failed to inspect applied migrations: {}", e)))?;

    let pending = migrator
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .filter(|migration| !applied.iter().any(|applied| applied.version == migration.version))
        .count();

    drop(conn);

    if pending > 0 {
        info!("Running {} migrations ({} pending)...", label, pending);
    }

    migrator
        .run(pool)
        .await
        .map_err(|e| crate::error::WrightError::DatabaseError(format!("failed to run migrations: {}", e)))?;

    Ok(())
}
