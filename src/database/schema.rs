use rusqlite::Connection;

use crate::error::{WrightError, Result};

pub fn init_db(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS packages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            version TEXT NOT NULL,
            release INTEGER NOT NULL,
            description TEXT,
            arch TEXT NOT NULL,
            license TEXT,
            url TEXT,
            installed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            install_size INTEGER,
            pkg_hash TEXT
        );

        CREATE TABLE IF NOT EXISTS files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            package_id INTEGER NOT NULL,
            path TEXT NOT NULL,
            file_hash TEXT,
            file_type TEXT NOT NULL,
            file_mode INTEGER,
            file_size INTEGER,
            is_config BOOLEAN DEFAULT 0,
            FOREIGN KEY (package_id) REFERENCES packages(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS dependencies (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            package_id INTEGER NOT NULL,
            depends_on TEXT NOT NULL,
            version_constraint TEXT,
            dep_type TEXT DEFAULT 'runtime',
            FOREIGN KEY (package_id) REFERENCES packages(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS transactions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
            operation TEXT NOT NULL,
            package_name TEXT NOT NULL,
            old_version TEXT,
            new_version TEXT,
            status TEXT NOT NULL,
            backup_path TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_files_path ON files(path);
        CREATE INDEX IF NOT EXISTS idx_files_package ON files(package_id);
        CREATE INDEX IF NOT EXISTS idx_deps_package ON dependencies(package_id);
        CREATE INDEX IF NOT EXISTS idx_deps_on ON dependencies(depends_on);
        ",
    )
    .map_err(|e| WrightError::DatabaseError(format!("failed to initialize database: {}", e)))?;

    // Migrate: add install_scripts column if missing
    let has_install_scripts = conn
        .prepare("SELECT install_scripts FROM packages LIMIT 0")
        .is_ok();
    if !has_install_scripts {
        conn.execute_batch("ALTER TABLE packages ADD COLUMN install_scripts TEXT")
            .map_err(|e| {
                WrightError::DatabaseError(format!(
                    "failed to add install_scripts column: {}",
                    e
                ))
            })?;
    }

    // Migrate: add dep_type column if missing
    let has_dep_type = conn
        .prepare("SELECT dep_type FROM dependencies LIMIT 0")
        .is_ok();
    if !has_dep_type {
        conn.execute_batch("ALTER TABLE dependencies ADD COLUMN dep_type TEXT DEFAULT 'runtime'")
            .map_err(|e| {
                WrightError::DatabaseError(format!(
                    "failed to add dep_type column: {}",
                    e
                ))
            })?;
    }

    // Enable foreign keys
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|e| WrightError::DatabaseError(format!("failed to enable foreign keys: {}", e)))?;

    Ok(())
}
