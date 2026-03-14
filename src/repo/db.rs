use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use crate::error::{Result, WrightError};
use crate::part::archive;

const REPO_DB_FILENAME: &str = "repo.db";

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS packages (
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
        package_id INTEGER NOT NULL,
        depends_on TEXT NOT NULL,
        dep_type TEXT NOT NULL DEFAULT 'runtime',
        FOREIGN KEY (package_id) REFERENCES packages(id) ON DELETE CASCADE
    );

    CREATE TABLE IF NOT EXISTS provides (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        package_id INTEGER NOT NULL,
        name TEXT NOT NULL,
        FOREIGN KEY (package_id) REFERENCES packages(id) ON DELETE CASCADE
    );

    CREATE TABLE IF NOT EXISTS conflicts (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        package_id INTEGER NOT NULL,
        name TEXT NOT NULL,
        FOREIGN KEY (package_id) REFERENCES packages(id) ON DELETE CASCADE
    );

    CREATE TABLE IF NOT EXISTS replaces (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        package_id INTEGER NOT NULL,
        name TEXT NOT NULL,
        FOREIGN KEY (package_id) REFERENCES packages(id) ON DELETE CASCADE
    );

    CREATE INDEX IF NOT EXISTS idx_repo_pkg_name ON packages(name);
    CREATE INDEX IF NOT EXISTS idx_repo_deps_pkg ON dependencies(package_id);
    CREATE INDEX IF NOT EXISTS idx_repo_deps_on ON dependencies(depends_on);
    CREATE INDEX IF NOT EXISTS idx_repo_provides_name ON provides(name);
    CREATE INDEX IF NOT EXISTS idx_repo_conflicts_name ON conflicts(name);
    CREATE INDEX IF NOT EXISTS idx_repo_replaces_name ON replaces(name);

    PRAGMA foreign_keys = ON;
";

#[derive(Debug, Clone)]
pub struct RepoPackage {
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
    pub link_deps: Vec<String>,
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

    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if ret != 0 {
        return Err(WrightError::DatabaseError(
            "repo database is locked by another process".to_string(),
        ));
    }

    Ok(file)
}

impl RepoDb {
    pub fn open(repo_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(repo_dir).map_err(|e| {
            WrightError::DatabaseError(format!(
                "failed to create repo directory {}: {}",
                repo_dir.display(),
                e
            ))
        })?;

        let db_path = repo_dir.join(REPO_DB_FILENAME);
        let lock_file = acquire_lock(&db_path)?;
        let conn = Connection::open(&db_path)?;

        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| WrightError::DatabaseError(format!("failed to enable WAL: {}", e)))?;

        conn.execute_batch(SCHEMA).map_err(|e| {
            WrightError::DatabaseError(format!("failed to init repo schema: {}", e))
        })?;

        Ok(RepoDb {
            conn,
            _lock_file: Some(lock_file),
            db_path: Some(db_path),
        })
    }

    /// Register a built package in the repo database.
    pub fn register_package(
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
            "DELETE FROM packages WHERE name = ?1 AND version = ?2 AND release = ?3 AND epoch = ?4",
            params![
                partinfo.name,
                partinfo.version,
                partinfo.release,
                partinfo.epoch
            ],
        )
        .map_err(|e| WrightError::DatabaseError(format!("delete old entry: {}", e)))?;

        tx.execute(
            "INSERT INTO packages (name, version, release, epoch, description, arch, license, filename, sha256, install_size, build_date)
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
        .map_err(|e| WrightError::DatabaseError(format!("insert package: {}", e)))?;

        let pkg_id = tx.last_insert_rowid();

        // Insert dependencies
        for dep in &partinfo.runtime_deps {
            tx.execute(
                "INSERT INTO dependencies (package_id, depends_on, dep_type) VALUES (?1, ?2, 'runtime')",
                params![pkg_id, dep],
            )
            .map_err(|e| WrightError::DatabaseError(format!("insert runtime dep: {}", e)))?;
        }
        for dep in &partinfo.link_deps {
            tx.execute(
                "INSERT INTO dependencies (package_id, depends_on, dep_type) VALUES (?1, ?2, 'link')",
                params![pkg_id, dep],
            )
            .map_err(|e| WrightError::DatabaseError(format!("insert link dep: {}", e)))?;
        }

        // Insert relations
        for name in &partinfo.provides {
            tx.execute(
                "INSERT INTO provides (package_id, name) VALUES (?1, ?2)",
                params![pkg_id, name],
            )
            .map_err(|e| WrightError::DatabaseError(format!("insert provides: {}", e)))?;
        }
        for name in &partinfo.conflicts {
            tx.execute(
                "INSERT INTO conflicts (package_id, name) VALUES (?1, ?2)",
                params![pkg_id, name],
            )
            .map_err(|e| WrightError::DatabaseError(format!("insert conflicts: {}", e)))?;
        }
        for name in &partinfo.replaces {
            tx.execute(
                "INSERT INTO replaces (package_id, name) VALUES (?1, ?2)",
                params![pkg_id, name],
            )
            .map_err(|e| WrightError::DatabaseError(format!("insert replaces: {}", e)))?;
        }

        tx.commit()
            .map_err(|e| WrightError::DatabaseError(format!("commit: {}", e)))?;

        Ok(pkg_id)
    }

    /// List packages, optionally filtered by name.
    pub fn list_packages(&self, name: Option<&str>) -> Result<Vec<RepoPackage>> {
        let mut packages = if let Some(name) = name {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT id, name, version, release, epoch, description, arch, filename, sha256, install_size
                     FROM packages WHERE name = ?1
                     ORDER BY epoch DESC, version DESC, release DESC",
                )
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            let rows = stmt
                .query_map(params![name], row_to_repo_package)
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?
                .filter_map(|r| r.ok())
                .collect::<Vec<_>>();
            rows
        } else {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT id, name, version, release, epoch, description, arch, filename, sha256, install_size
                     FROM packages ORDER BY name, epoch DESC, version DESC, release DESC",
                )
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            let rows = stmt
                .query_map([], row_to_repo_package)
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?
                .filter_map(|r| r.ok())
                .collect::<Vec<_>>();
            rows
        };

        // Fill in dependencies
        for pkg in &mut packages {
            pkg.runtime_deps = self.get_deps(pkg.id, "runtime")?;
            pkg.link_deps = self.get_deps(pkg.id, "link")?;
        }

        Ok(packages)
    }

    /// Search packages by keyword (matches name and description).
    pub fn search_packages(&self, keyword: &str) -> Result<Vec<RepoPackage>> {
        let pattern = format!("%{}%", keyword);
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, version, release, epoch, description, arch, filename, sha256, install_size
                 FROM packages
                 WHERE name LIKE ?1 OR description LIKE ?1
                 ORDER BY name, epoch DESC, version DESC, release DESC",
            )
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?;

        let mut packages: Vec<RepoPackage> = stmt
            .query_map(params![pattern], row_to_repo_package)
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        for pkg in &mut packages {
            pkg.runtime_deps = self.get_deps(pkg.id, "runtime")?;
            pkg.link_deps = self.get_deps(pkg.id, "link")?;
        }

        Ok(packages)
    }

    /// Find the latest version of a package by name.
    pub fn find_package(&self, name: &str) -> Result<Option<RepoPackage>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, version, release, epoch, description, arch, filename, sha256, install_size
                 FROM packages WHERE name = ?1
                 ORDER BY epoch DESC, version DESC, release DESC
                 LIMIT 1",
            )
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?;

        let mut pkg = stmt
            .query_map(params![name], row_to_repo_package)
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?
            .filter_map(|r| r.ok())
            .next();

        if let Some(ref mut p) = pkg {
            p.runtime_deps = self.get_deps(p.id, "runtime")?;
            p.link_deps = self.get_deps(p.id, "link")?;
        }

        Ok(pkg)
    }

    /// Find all versions of a package.
    pub fn find_all_versions(&self, name: &str) -> Result<Vec<RepoPackage>> {
        self.list_packages(Some(name))
    }

    /// Remove a package entry from the repo database.
    pub fn remove_package(
        &self,
        name: &str,
        version: &str,
        release: Option<u32>,
    ) -> Result<Vec<(String, String, u32)>> {
        let entries: Vec<(i64, String, String, u32)> = if let Some(rel) = release {
            let mut stmt = self
                .conn
                .prepare("SELECT id, name, version, release FROM packages WHERE name = ?1 AND version = ?2 AND release = ?3")
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
                .prepare("SELECT id, name, version, release FROM packages WHERE name = ?1 AND version = ?2")
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
                .execute("DELETE FROM packages WHERE id = ?1", params![id])
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            removed.push((n, v, r));
        }

        Ok(removed)
    }

    /// Get the filename for a specific package version.
    pub fn get_filename(
        &self,
        name: &str,
        version: &str,
        release: Option<u32>,
    ) -> Result<Option<String>> {
        let result = if let Some(rel) = release {
            self.conn
                .query_row(
                    "SELECT filename FROM packages WHERE name = ?1 AND version = ?2 AND release = ?3",
                    params![name, version, rel],
                    |row| row.get(0),
                )
                .ok()
        } else {
            self.conn
                .query_row(
                    "SELECT filename FROM packages WHERE name = ?1 AND version = ?2 ORDER BY release DESC LIMIT 1",
                    params![name, version],
                    |row| row.get(0),
                )
                .ok()
        };
        Ok(result)
    }

    /// Bulk-import packages from a directory of `.wright.tar.zst` archives.
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
            self.register_package(&partinfo, fname, &sha256)?;
            count += 1;
        }

        Ok(count)
    }

    /// Get the total number of packages in the repo.
    pub fn package_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM packages", [], |row| row.get(0))
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
        Ok(count as usize)
    }

    fn get_deps(&self, package_id: i64, dep_type: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT depends_on FROM dependencies WHERE package_id = ?1 AND dep_type = ?2")
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
        let deps = stmt
            .query_map(params![package_id, dep_type], |row| row.get(0))
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(deps)
    }
}

fn row_to_repo_package(row: &rusqlite::Row) -> rusqlite::Result<RepoPackage> {
    Ok(RepoPackage {
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
        link_deps: Vec::new(),
    })
}

impl Drop for RepoDb {
    fn drop(&mut self) {
        if let Some(ref path) = self.db_path {
            let _ = std::fs::remove_file(path.with_extension("lock"));
        }
    }
}
