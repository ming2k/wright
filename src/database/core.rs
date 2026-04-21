use std::path::{Path, PathBuf};
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use crate::error::{Result, WrightError};
use crate::util::lock::ProcessLock;
use super::schema;

pub struct InstalledDb {
    pub(crate) pool: SqlitePool,
    pub(super) _lock: Option<ProcessLock>,
    pub(super) db_path: Option<PathBuf>,
}

pub(super) const PART_COLUMNS: &str =
    "id, name, version, release, epoch, description, arch, license, url, installed_at, install_size, part_hash, install_scripts, assumed, origin";

fn acquire_lock(db_path: &Path) -> Result<ProcessLock> {
    let file_name = db_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("database");

    crate::util::lock::acquire_lock(
        &crate::util::lock::lock_dir_from_db(db_path),
        crate::util::lock::LockIdentity::Database(file_name),
        crate::util::lock::LockMode::Exclusive,
    )
    .map_err(|e| WrightError::DatabaseError(e.to_string()))
}

impl InstalledDb {
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                WrightError::DatabaseError(format!(
                    "failed to create database directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }

        let lock_file = acquire_lock(path)?;
        
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

        let pool = SqlitePool::connect_with(options)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to connect to database: {}", e)))?;

        schema::init_db(&pool).await?;

        Ok(InstalledDb {
            pool,
            _lock: Some(lock_file),
            db_path: Some(path.to_path_buf()),
        })
    }

    pub async fn open_in_memory() -> Result<Self> {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to connect to in-memory database: {}", e)))?;
        
        schema::init_db(&pool).await?;
        
        Ok(InstalledDb {
            pool,
            _lock: None,
            db_path: None,
        })
    }
}
