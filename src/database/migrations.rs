use sqlx::SqlitePool;
use tracing::info;
use crate::error::Result;

pub async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    info!("Running database migrations...");
    
    sqlx::migrate!("./src/database/migrations")
        .run(pool)
        .await
        .map_err(|e| crate::error::WrightError::DatabaseError(format!("failed to run migrations: {}", e)))?;

    Ok(())
}
