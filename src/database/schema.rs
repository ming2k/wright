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
            pkg_hash TEXT,
            install_scripts TEXT,
            assumed INTEGER NOT NULL DEFAULT 0
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

        CREATE INDEX IF NOT EXISTS idx_files_package ON files(package_id);
        CREATE INDEX IF NOT EXISTS idx_deps_package ON dependencies(package_id);
        CREATE INDEX IF NOT EXISTS idx_deps_on ON dependencies(depends_on);

        -- Shadowed files (for conflict analysis and safe removal)
        CREATE TABLE IF NOT EXISTS shadowed_files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL,
            original_owner_id INTEGER NOT NULL,
            shadowed_by_id INTEGER NOT NULL,
            timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (original_owner_id) REFERENCES packages(id) ON DELETE CASCADE,
            FOREIGN KEY (shadowed_by_id) REFERENCES packages(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_shadowed_path ON shadowed_files(path);
        ",
    )
    .map_err(|e| WrightError::DatabaseError(format!("failed to initialize database: {}", e)))?;

    // Migration: add assumed column to databases created before this feature.
    let _ = conn.execute_batch(
        "ALTER TABLE packages ADD COLUMN assumed INTEGER NOT NULL DEFAULT 0;",
    );

    conn.execute_batch("

        -- Optional (informational) dependencies, not enforced
        CREATE TABLE IF NOT EXISTS optional_dependencies (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            package_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            description TEXT,
            FOREIGN KEY (package_id) REFERENCES packages(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_opt_deps_package ON optional_dependencies(package_id);
        ",
    )
    .map_err(|e| WrightError::DatabaseError(format!("failed to initialize database: {}", e)))?;

    // Enable foreign keys
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|e| WrightError::DatabaseError(format!("failed to enable foreign keys: {}", e)))?;

    Ok(())
}
