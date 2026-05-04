use tempfile::tempdir;
use wright::database::InstalledDb;

use sqlx::sqlite::SqliteConnectOptions;
use std::path::Path;

const INSTALLED_SCHEMA: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/database/migrations/001_initial_schema.sql"
));

async fn seed_schema_without_sqlx_migrations(path: &Path, schema: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }

    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);

    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    sqlx::raw_sql(schema).execute(&pool).await.unwrap();
    pool.close().await;
}

async fn migration_count(path: &Path) -> i64 {
    let options = SqliteConnectOptions::new().filename(path);
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    let count =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations WHERE version = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    pool.close().await;
    count
}

#[tokio::test]
async fn installed_db_open_handles_preseeded_v1_schema_without_sqlx_metadata() {
    let temp = tempdir().unwrap();
    let db_path = temp.path().join("wright").join("wright.db");

    seed_schema_without_sqlx_migrations(&db_path, INSTALLED_SCHEMA).await;

    let db = InstalledDb::open(&db_path).await;
    assert!(db.is_ok(), "InstalledDb::open failed: {:?}", db.err());
    drop(db);

    assert_eq!(migration_count(&db_path).await, 1);
}
