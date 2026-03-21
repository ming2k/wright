use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use crate::error::{Result, WrightError};
use crate::part::archive;

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
    CREATE INDEX IF NOT EXISTS idx_repo_pkg_name ON parts(name);
    CREATE INDEX IF NOT EXISTS idx_repo_deps_pkg ON dependencies(part_id);
    CREATE INDEX IF NOT EXISTS idx_repo_deps_on ON dependencies(depends_on);
    CREATE INDEX IF NOT EXISTS idx_repo_provides_name ON provides(name);
    CREATE INDEX IF NOT EXISTS idx_repo_conflicts_name ON conflicts(name);
    CREATE INDEX IF NOT EXISTS idx_repo_replaces_name ON replaces(name);
";

#[derive(Debug, Clone)]
pub struct RepoPart {
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

pub struct RepoDb {
    conn: Connection,
    _lock_file: Option<File>,
    db_path: Option<PathBuf>,
}

fn acquire_lock(db_path: &Path) -> Result<File> {
    let lock_path = db_path.with_extension("lock");
    let file = File::create(&lock_path).map_err(|e| {
        WrightError::DatabaseError(format!(
            "failed to create repo lock file {}: {}",
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
                "repo database is locked by another process (timed out after 30s)".to_string(),
            ));
        }
        std::thread::sleep(delay);
        delay = (delay * 2).min(std::time::Duration::from_secs(1));
    }
}

fn repo_schema_error(db_path: &Path, err: rusqlite::Error) -> String {
    format!(
        "failed to open repo database {} with the current schema: {}. \
If this is an older repo.db, remove it and rebuild the index with `wrepo sync`.",
        db_path.display(),
        err
    )
}

impl RepoDb {
    pub fn open(db_path: &Path) -> Result<Self> {
        let parent = db_path.parent().ok_or_else(|| {
            WrightError::DatabaseError(format!(
                "repo database path has no parent directory: {}",
                db_path.display()
            ))
        })?;
        std::fs::create_dir_all(parent).map_err(|e| {
            WrightError::DatabaseError(format!(
                "failed to create repo database directory {}: {}",
                parent.display(),
                e
            ))
        })?;

        let lock_file = acquire_lock(db_path)?;
        let conn = Connection::open(db_path)?;

        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| WrightError::DatabaseError(format!("failed to enable WAL: {}", e)))?;

        conn.execute_batch(BASE_SCHEMA)
            .map_err(|e| WrightError::DatabaseError(repo_schema_error(db_path, e)))?;
        conn.execute_batch(INDEX_SCHEMA)
            .map_err(|e| WrightError::DatabaseError(repo_schema_error(db_path, e)))?;

        Ok(RepoDb {
            conn,
            _lock_file: Some(lock_file),
            db_path: Some(db_path.to_path_buf()),
        })
    }

    /// Register a built part in the repo database.
    pub fn register_part(
        &self,
        partinfo: &archive::PartInfo,
        filename: &str,
        sha256: &str,
    ) -> Result<i64> {
        let tx = self.conn.unchecked_transaction().map_err(|e| {
            WrightError::DatabaseError(format!("failed to begin transaction: {}", e))
        })?;

        // Delete existing entry for same name/version/release/epoch
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

        // Insert dependencies
        for dep in &partinfo.runtime_deps {
            tx.execute(
                "INSERT INTO dependencies (part_id, depends_on, dep_type) VALUES (?1, ?2, 'runtime')",
                params![pkg_id, dep],
            )
            .map_err(|e| WrightError::DatabaseError(format!("insert runtime dep: {}", e)))?;
        }
        // Insert relations
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

    /// List parts, optionally filtered by name.
    pub fn list_parts(&self, name: Option<&str>) -> Result<Vec<RepoPart>> {
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
                .query_map(params![name], row_to_repo_part)
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
                .query_map([], row_to_repo_part)
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?
                .filter_map(|r| r.ok())
                .collect::<Vec<_>>();
            rows
        };

        // Fill in dependencies
        for pkg in &mut parts {
            pkg.runtime_deps = self.get_deps(pkg.id, "runtime")?;
        }

        Ok(parts)
    }

    /// Search parts by keyword (matches name and description).
    pub fn search_parts(&self, keyword: &str) -> Result<Vec<RepoPart>> {
        let pattern = format!("%{}%", keyword);
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, version, release, epoch, description, arch, filename, sha256, install_size
                 FROM parts
                 WHERE name LIKE ?1 OR description LIKE ?1
                 ORDER BY name, epoch DESC, version DESC, release DESC",
            )
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?;

        let mut parts: Vec<RepoPart> = stmt
            .query_map(params![pattern], row_to_repo_part)
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        for pkg in &mut parts {
            pkg.runtime_deps = self.get_deps(pkg.id, "runtime")?;
        }

        Ok(parts)
    }

    /// Find the latest version of a part by name.
    pub fn find_part(&self, name: &str) -> Result<Option<RepoPart>> {
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
            .query_map(params![name], row_to_repo_part)
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?
            .filter_map(|r| r.ok())
            .next();

        if let Some(ref mut p) = pkg {
            p.runtime_deps = self.get_deps(p.id, "runtime")?;
        }

        Ok(pkg)
    }

    /// Find all versions of a part.
    pub fn find_all_versions(&self, name: &str) -> Result<Vec<RepoPart>> {
        self.list_parts(Some(name))
    }

    /// Remove a part entry from the repo database.
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

    /// Get the filename for a specific part version.
    pub fn get_filename(
        &self,
        name: &str,
        version: &str,
        release: Option<u32>,
    ) -> Result<Option<String>> {
        let result = if let Some(rel) = release {
            self.conn
                .query_row(
                    "SELECT filename FROM parts WHERE name = ?1 AND version = ?2 AND release = ?3",
                    params![name, version, rel],
                    |row| row.get(0),
                )
                .ok()
        } else {
            self.conn
                .query_row(
                    "SELECT filename FROM parts WHERE name = ?1 AND version = ?2 ORDER BY release DESC LIMIT 1",
                    params![name, version],
                    |row| row.get(0),
                )
                .ok()
        };
        Ok(result)
    }

    /// Bulk-import parts from a directory of `.wright.tar.zst` archives.
    pub fn sync_from_archives(&self, dir: &Path) -> Result<usize> {
        if !dir.exists() {
            return Ok(0);
        }

        let mut count = 0;
        for entry in std::fs::read_dir(dir).map_err(WrightError::IoError)? {
            let entry = entry.map_err(WrightError::IoError)?;
            let path = entry.path();

            if !path.is_file() {
                continue;
            }
            let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if !fname.ends_with(".wright.tar.zst") {
                continue;
            }

            let partinfo = match archive::read_partinfo(&path) {
                Ok(info) => info,
                Err(e) => {
                    tracing::warn!("Skipping {}: {}", path.display(), e);
                    continue;
                }
            };

            let sha256 = crate::util::checksum::sha256_file(&path)?;
            self.register_part(&partinfo, fname, &sha256)?;
            count += 1;
        }

        Ok(count)
    }

    /// Get the total number of parts in the repo.
    pub fn package_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM parts", [], |row| row.get(0))
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
        Ok(count as usize)
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

fn row_to_repo_part(row: &rusqlite::Row) -> rusqlite::Result<RepoPart> {
    Ok(RepoPart {
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

impl Drop for RepoDb {
    fn drop(&mut self) {
        if let Some(ref path) = self.db_path {
            let _ = std::fs::remove_file(path.with_extension("lock"));
        }
    }
}
