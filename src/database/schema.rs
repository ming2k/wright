use sqlx::SqlitePool;
use crate::error::Result;
use super::migrations::run_migrations;

pub async fn init_db(pool: &SqlitePool) -> Result<()> {
    // Run migrations to ensure database is at the latest version
    run_migrations(pool).await?;

    // Enable foreign keys is usually done via PRAGMA, but sqlx handles connection pooling.
    // We can set it in the connection options if needed, but for now we'll do it explicitly if required.
    // However, sqlx::migrate! usually handles its own setup.
    
    Ok(())
}
