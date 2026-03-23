use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::error::{Result, WrightError};

use super::{schema, InstalledPart, Origin, TransactionRecord};

pub struct Database {
    pub(super) conn: Connection,
    pub(super) _lock_file: Option<File>,
    pub(super) db_path: Option<PathBuf>,
}

pub(super) const PART_COLUMNS: &str =
    "id, name, version, release, description, arch, license, url, installed_at, install_size, pkg_hash, install_scripts, assumed, origin, epoch";

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
        pkg_hash: row.get(10)?,
        install_scripts: row.get(11)?,
        assumed: row.get::<_, bool>(12)?,
        origin: Origin::try_from(row.get::<_, String>(13)?.as_str()).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(13, rusqlite::types::Type::Text, Box::new(e))
        })?,
        epoch: row.get::<_, u32>(14).unwrap_or(0),
    })
}

fn acquire_lock(db_path: &Path) -> Result<File> {
    let lock_path = db_path.with_extension("lock");
    let file = File::create(&lock_path).map_err(|e| {
        WrightError::DatabaseError(format!(
            "failed to create lock file {}: {}",
            lock_path.display(),
            e
        ))
    })?;

    let mut delay = std::time::Duration::from_millis(50);
    let max_wait = std::time::Duration::from_secs(30);
    let start = std::time::Instant::now();

    loop {
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret == 0 {
            return Ok(file);
        }
        if start.elapsed() >= max_wait {
            return Err(WrightError::DatabaseError(
                "database is locked by another process (timed out after 30s)".to_string(),
            ));
        }
        std::thread::sleep(delay);
        delay = (delay * 2).min(std::time::Duration::from_secs(1));
    }
}

impl Database {
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

        Ok(Database {
            conn,
            _lock_file: Some(lock_file),
            db_path: Some(path.to_path_buf()),
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        schema::init_db(&conn)?;
        Ok(Database {
            conn,
            _lock_file: None,
            db_path: None,
        })
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        if let Some(ref path) = self.db_path {
            let _ = std::fs::remove_file(path.with_extension("lock"));
        }
    }
}
