use tempfile::tempdir;
use wright::database::InstalledDb;

use std::path::Path;

const INSTALLED_SCHEMA: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/crates/wright-state/migrations/001_initial_schema.sql"
));

fn seed_preseeded_v1_schema(path: &Path, schema: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }

    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(schema).unwrap();
}

fn get_user_version(path: &Path) -> u32 {
    let conn = rusqlite::Connection::open(path).unwrap();
    let version: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    version
}

#[tokio::test]
async fn installed_db_open_handles_preseeded_v1_schema() {
    let temp = tempdir().unwrap();
    let db_path = temp.path().join("wright").join("wright.db");

    seed_preseeded_v1_schema(&db_path, INSTALLED_SCHEMA);

    let db = InstalledDb::open(&db_path, None).await;
    assert!(db.is_ok(), "InstalledDb::open failed: {:?}", db.err());
    drop(db);

    assert_eq!(get_user_version(&db_path), 20);
}

#[tokio::test]
async fn installed_db_open_migrates_legacy_sqlx_database() {
    let temp = tempdir().unwrap();
    let db_path = temp.path().join("wright").join("wright.db");

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }

    // Seed up to migration 19 with a legacy _sqlx_migrations table
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE _sqlx_migrations (
            version INTEGER PRIMARY KEY,
            description TEXT NOT NULL,
            installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success BOOLEAN NOT NULL,
            checksum BLOB NOT NULL,
            execution_time INTEGER NOT NULL
        );
        INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
        VALUES (19, '019_normalize_dependency_edges.sql', 1, X'', 0);",
    )
    .unwrap();
    drop(conn);

    let db = InstalledDb::open(&db_path, None).await;
    assert!(db.is_ok(), "InstalledDb::open failed: {:?}", db.err());
    drop(db);

    // Database should have recognized version 19 from _sqlx_migrations and migrated to 20
    assert_eq!(get_user_version(&db_path), 20);
}
