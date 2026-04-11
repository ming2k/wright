use std::path::Path;

use rusqlite::{params, Connection};

use crate::error::{Result, WrightError};
use crate::part::archive;
use crate::util::lock::ProcessLock;

const BASE_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS parts (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL,
        version TEXT NOT NULL,
        release INTEGER NOT NULL DEFAULT 1,
        epoch INTEGER NOT NULL DEFAULT 0,
        description TEXT NOT NULL DEFAULT '',
        arch TEXT NOT NULL DEFAULT 'x86_64',
        license TEXT NOT NULL DEFAULT '',
        filename TEXT NOT NULL,
        sha256 TEXT NOT NULL DEFAULT '',
        install_size INTEGER NOT NULL DEFAULT 0,
        build_date TEXT,
        registered_at DATETIME DEFAULT CURRENT_TIMESTAMP,
        UNIQUE(name, version, release, epoch)
    );

    CREATE TABLE IF NOT EXISTS dependencies (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        part_id INTEGER NOT NULL,
        depends_on TEXT NOT NULL,
        dep_type TEXT NOT NULL DEFAULT 'runtime',
        FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
    );

    CREATE TABLE IF NOT EXISTS provides (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        part_id INTEGER NOT NULL,
        name TEXT NOT NULL,
        FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
    );

    CREATE TABLE IF NOT EXISTS conflicts (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        part_id INTEGER NOT NULL,
        name TEXT NOT NULL,
        FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
    );

    CREATE TABLE IF NOT EXISTS replaces (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        part_id INTEGER NOT NULL,
        name TEXT NOT NULL,
        FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
    );

    PRAGMA foreign_keys = ON;
";

const INDEX_SCHEMA: &str = "
    CREATE INDEX IF NOT EXISTS idx_inventory_pkg_name ON parts(name);
    CREATE INDEX IF NOT EXISTS idx_inventory_pkg_filename ON parts(filename);
    CREATE INDEX IF NOT EXISTS idx_inventory_deps_pkg ON dependencies(part_id);
    CREATE INDEX IF NOT EXISTS idx_inventory_deps_on ON dependencies(depends_on);
    CREATE INDEX IF NOT EXISTS idx_inventory_provides_name ON provides(name);
    CREATE INDEX IF NOT EXISTS idx_inventory_conflicts_name ON conflicts(name);
    CREATE INDEX IF NOT EXISTS idx_inventory_replaces_name ON replaces(name);
";

#[derive(Debug, Clone)]
pub struct InventoryPart {
    pub id: i64,
    pub name: String,
    pub version: String,
    pub release: u32,
    pub epoch: u32,
    pub description: String,
    pub arch: String,
    pub filename: String,
    pub sha256: String,
    pub install_size: u64,
    pub runtime_deps: Vec<String>,
}

pub struct InventoryDb {
    conn: Connection,
    _lock: Option<ProcessLock>,
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

fn inventory_schema_error(db_path: &Path, err: rusqlite::Error) -> String {
    format!(
        "failed to open inventory database {} with the current schema: {}",
        db_path.display(),
        err
    )
}

impl InventoryDb {
    pub fn open(db_path: &Path) -> Result<Self> {
        let parent = db_path.parent().ok_or_else(|| {
            WrightError::DatabaseError(format!(
                "inventory database path has no parent directory: {}",
                db_path.display()
            ))
        })?;
        std::fs::create_dir_all(parent).map_err(|e| {
            WrightError::DatabaseError(format!(
                "failed to create inventory database directory {}: {}",
                parent.display(),
                e
            ))
        })?;

        let lock_file = acquire_lock(db_path)?;
        let conn = Connection::open(db_path)?;

        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| WrightError::DatabaseError(format!("failed to enable WAL: {}", e)))?;

        conn.execute_batch(BASE_SCHEMA)
            .map_err(|e| WrightError::DatabaseError(inventory_schema_error(db_path, e)))?;
        conn.execute_batch(INDEX_SCHEMA)
            .map_err(|e| WrightError::DatabaseError(inventory_schema_error(db_path, e)))?;

        Ok(Self {
            conn,
            _lock: Some(lock_file),
        })
    }

    pub fn register_part(
        &self,
        partinfo: &archive::PartInfo,
        filename: &str,
        sha256: &str,
    ) -> Result<i64> {
        let tx = self.conn.unchecked_transaction().map_err(|e| {
            WrightError::DatabaseError(format!("failed to begin transaction: {}", e))
        })?;

        tx.execute(
            "DELETE FROM parts WHERE name = ?1 AND version = ?2 AND release = ?3 AND epoch = ?4",
            params![
                partinfo.name,
                partinfo.version,
                partinfo.release,
                partinfo.epoch
            ],
        )
        .map_err(|e| WrightError::DatabaseError(format!("delete old entry: {}", e)))?;

        tx.execute(
            "INSERT INTO parts (name, version, release, epoch, description, arch, license, filename, sha256, install_size, build_date)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                partinfo.name,
                partinfo.version,
                partinfo.release,
                partinfo.epoch,
                partinfo.description,
                partinfo.arch,
                partinfo.license,
                filename,
                sha256,
                partinfo.install_size,
                partinfo.build_date,
            ],
        )
        .map_err(|e| WrightError::DatabaseError(format!("insert part: {}", e)))?;

        let pkg_id = tx.last_insert_rowid();

        for dep in &partinfo.runtime_deps {
            tx.execute(
                "INSERT INTO dependencies (part_id, depends_on, dep_type) VALUES (?1, ?2, 'runtime')",
                params![pkg_id, dep],
            )
            .map_err(|e| WrightError::DatabaseError(format!("insert runtime dep: {}", e)))?;
        }
        for name in &partinfo.provides {
            tx.execute(
                "INSERT INTO provides (part_id, name) VALUES (?1, ?2)",
                params![pkg_id, name],
            )
            .map_err(|e| WrightError::DatabaseError(format!("insert provides: {}", e)))?;
        }
        for name in &partinfo.conflicts {
            tx.execute(
                "INSERT INTO conflicts (part_id, name) VALUES (?1, ?2)",
                params![pkg_id, name],
            )
            .map_err(|e| WrightError::DatabaseError(format!("insert conflicts: {}", e)))?;
        }
        for name in &partinfo.replaces {
            tx.execute(
                "INSERT INTO replaces (part_id, name) VALUES (?1, ?2)",
                params![pkg_id, name],
            )
            .map_err(|e| WrightError::DatabaseError(format!("insert replaces: {}", e)))?;
        }

        tx.commit()
            .map_err(|e| WrightError::DatabaseError(format!("commit: {}", e)))?;

        Ok(pkg_id)
    }

    pub fn list_parts(&self, name: Option<&str>) -> Result<Vec<InventoryPart>> {
        let mut parts = if let Some(name) = name {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT id, name, version, release, epoch, description, arch, filename, sha256, install_size
                     FROM parts WHERE name = ?1
                     ORDER BY epoch DESC, version DESC, release DESC",
                )
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            let rows = stmt
                .query_map(params![name], row_to_inventory_part)
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?
                .filter_map(|r| r.ok())
                .collect::<Vec<_>>();
            rows
        } else {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT id, name, version, release, epoch, description, arch, filename, sha256, install_size
                     FROM parts ORDER BY name, epoch DESC, version DESC, release DESC",
                )
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            let rows = stmt
                .query_map([], row_to_inventory_part)
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?
                .filter_map(|r| r.ok())
                .collect::<Vec<_>>();
            rows
        };

        for pkg in &mut parts {
            pkg.runtime_deps = self.get_deps(pkg.id, "runtime")?;
        }

        Ok(parts)
    }

    pub fn find_part(&self, name: &str) -> Result<Option<InventoryPart>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, version, release, epoch, description, arch, filename, sha256, install_size
                 FROM parts WHERE name = ?1
                 ORDER BY epoch DESC, version DESC, release DESC
                 LIMIT 1",
            )
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?;

        let mut pkg = stmt
            .query_map(params![name], row_to_inventory_part)
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?
            .filter_map(|r| r.ok())
            .next();

        if let Some(ref mut p) = pkg {
            p.runtime_deps = self.get_deps(p.id, "runtime")?;
        }

        Ok(pkg)
    }

    pub fn find_all_versions(&self, name: &str) -> Result<Vec<InventoryPart>> {
        self.list_parts(Some(name))
    }

    pub fn remove_part(
        &self,
        name: &str,
        version: &str,
        release: Option<u32>,
    ) -> Result<Vec<(String, String, u32)>> {
        let entries: Vec<(i64, String, String, u32)> = if let Some(rel) = release {
            let mut stmt = self
                .conn
                .prepare("SELECT id, name, version, release FROM parts WHERE name = ?1 AND version = ?2 AND release = ?3")
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            let rows = stmt
                .query_map(params![name, version, rel], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?
                .filter_map(|r| r.ok())
                .collect();
            rows
        } else {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT id, name, version, release FROM parts WHERE name = ?1 AND version = ?2",
                )
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            let rows = stmt
                .query_map(params![name, version], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?
                .filter_map(|r| r.ok())
                .collect();
            rows
        };

        let mut removed = Vec::new();
        for (id, n, v, r) in entries {
            self.conn
                .execute("DELETE FROM parts WHERE id = ?1", params![id])
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            removed.push((n, v, r));
        }

        Ok(removed)
    }

    pub fn remove_missing_files(&self, dir: &Path) -> Result<Vec<String>> {
        let filenames = self.list_filenames()?;
        let mut removed = Vec::new();
        for filename in filenames {
            if !dir.join(&filename).exists() {
                self.conn
                    .execute("DELETE FROM parts WHERE filename = ?1", params![filename])
                    .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
                removed.push(filename);
            }
        }
        Ok(removed)
    }

    pub fn list_filenames(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT filename FROM parts ORDER BY filename")
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
        let filenames = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect::<Vec<_>>();
        Ok(filenames)
    }

    fn get_deps(&self, part_id: i64, dep_type: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT depends_on FROM dependencies WHERE part_id = ?1 AND dep_type = ?2")
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
        let deps = stmt
            .query_map(params![part_id, dep_type], |row| row.get(0))
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(deps)
    }
}

fn row_to_inventory_part(row: &rusqlite::Row) -> rusqlite::Result<InventoryPart> {
    Ok(InventoryPart {
        id: row.get(0)?,
        name: row.get(1)?,
        version: row.get(2)?,
        release: row.get::<_, u32>(3)?,
        epoch: row.get::<_, u32>(4)?,
        description: row.get::<_, String>(5)?,
        arch: row.get(6)?,
        filename: row.get(7)?,
        sha256: row.get(8)?,
        install_size: row.get::<_, u64>(9)?,
        runtime_deps: Vec::new(),
    })
}
