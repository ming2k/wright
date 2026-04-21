use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::error::{Result, WrightError};
use crate::util::lock::ProcessLock;

use super::{schema, InstalledPart, Origin, TransactionRecord};

pub struct InstalledDb {
    pub(super) conn: Connection,
    pub(super) _lock: Option<ProcessLock>,
    pub(super) db_path: Option<PathBuf>,
}

pub(super) const PART_COLUMNS: &str =
    "id, name, version, release, description, arch, license, url, installed_at, install_size, part_hash, install_scripts, assumed, origin, epoch";

pub(super) fn row_to_transaction(row: &rusqlite::Row) -> rusqlite::Result<TransactionRecord> {
    Ok(TransactionRecord {
        timestamp: row.get(0)?,
        operation: row.get(1)?,
        part_name: row.get(2)?,
        old_version: row.get(3)?,
        new_version: row.get(4)?,
        status: row.get(5)?,
    })
}

pub(super) fn row_to_installed_part(row: &rusqlite::Row) -> rusqlite::Result<InstalledPart> {
    Ok(InstalledPart {
        id: row.get(0)?,
        name: row.get(1)?,
        version: row.get(2)?,
        release: row.get::<_, u32>(3)?,
        description: row.get::<_, String>(4)?,
        arch: row.get(5)?,
        license: row.get::<_, String>(6)?,
        url: row.get(7)?,
        installed_at: row.get::<_, String>(8)?,
        install_size: row.get::<_, u64>(9)?,
        part_hash: row.get(10)?,
        install_scripts: row.get(11)?,
        assumed: row.get::<_, bool>(12)?,
        origin: Origin::try_from(row.get::<_, String>(13)?.as_str()).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(13, rusqlite::types::Type::Text, Box::new(e))
        })?,
        epoch: row.get::<_, u32>(14).unwrap_or(0),
    })
}

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
    pub fn open(path: &Path) -> Result<Self> {
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
        let conn = Connection::open(path)?;

        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| WrightError::DatabaseError(format!("failed to enable WAL: {}", e)))?;

        schema::init_db(&conn)?;

        Ok(InstalledDb {
            conn,
            _lock: Some(lock_file),
            db_path: Some(path.to_path_buf()),
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        schema::init_db(&conn)?;
        Ok(InstalledDb {
            conn,
            _lock: None,
            db_path: None,
        })
    }
}
