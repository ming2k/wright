pub mod schema;

use rusqlite::{params, Connection};
use std::collections::HashSet;
use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use crate::error::{Result, WrightError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    File,
    Symlink,
    Directory,
}

impl FileType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Symlink => "symlink",
            Self::Directory => "dir",
        }
    }
}

impl TryFrom<&str> for FileType {
    type Error = WrightError;
    fn try_from(s: &str) -> Result<Self> {
        match s {
            "file" => Ok(Self::File),
            "symlink" => Ok(Self::Symlink),
            "dir" => Ok(Self::Directory),
            _ => Err(WrightError::DatabaseError(format!(
                "unknown file type: {}",
                s
            ))),
        }
    }
}

impl rusqlite::ToSql for FileType {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::Borrowed(
            rusqlite::types::ValueRef::Text(self.as_str().as_bytes()),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepType {
    Runtime,
    Link,
    Build,
}

impl DepType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Link => "link",
            Self::Build => "build",
        }
    }
}

impl TryFrom<&str> for DepType {
    type Error = WrightError;
    fn try_from(s: &str) -> Result<Self> {
        match s {
            "runtime" => Ok(Self::Runtime),
            "link" => Ok(Self::Link),
            "build" => Ok(Self::Build),
            _ => Err(WrightError::DatabaseError(format!(
                "unknown dep type: {}",
                s
            ))),
        }
    }
}

impl rusqlite::ToSql for DepType {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::Borrowed(
            rusqlite::types::ValueRef::Text(self.as_str().as_bytes()),
        ))
    }
}

#[derive(Debug, Clone)]
pub struct InstalledPart {
    pub id: i64,
    pub name: String,
    pub version: String,
    pub release: u32,
    pub epoch: u32,
    pub description: String,
    pub arch: String,
    pub license: String,
    pub url: Option<String>,
    pub installed_at: String,
    pub install_size: u64,
    pub pkg_hash: Option<String>,
    pub install_scripts: Option<String>,
    pub assumed: bool,
    pub install_reason: String,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: String,
    pub file_hash: Option<String>,
    pub file_type: FileType,
    pub file_mode: Option<u32>,
    pub file_size: Option<u64>,
    pub is_config: bool,
}

#[derive(Debug, Clone)]
pub struct NewPart<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub release: u32,
    pub epoch: u32,
    pub description: &'a str,
    pub arch: &'a str,
    pub license: &'a str,
    pub url: Option<&'a str>,
    pub install_size: u64,
    pub pkg_hash: Option<&'a str>,
    pub install_scripts: Option<&'a str>,
    pub install_reason: &'a str,
}

impl<'a> Default for NewPart<'a> {
    fn default() -> Self {
        Self {
            name: "",
            version: "",
            release: 0,
            epoch: 0,
            description: "",
            arch: "",
            license: "",
            url: None,
            install_size: 0,
            pkg_hash: None,
            install_scripts: None,
            install_reason: "explicit",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: String,
    pub constraint: Option<String>,
    pub dep_type: DepType,
}

#[derive(Debug, Clone)]
pub struct TransactionRecord {
    pub timestamp: String,
    pub operation: String,
    pub part_name: String,
    pub old_version: Option<String>,
    pub new_version: Option<String>,
    pub status: String,
}

fn row_to_transaction(row: &rusqlite::Row) -> rusqlite::Result<TransactionRecord> {
    Ok(TransactionRecord {
        timestamp: row.get(0)?,
        operation: row.get(1)?,
        part_name: row.get(2)?,
        old_version: row.get(3)?,
        new_version: row.get(4)?,
        status: row.get(5)?,
    })
}

pub struct Database {
    conn: Connection,
    _lock_file: Option<File>,
    db_path: Option<PathBuf>,
}

/// Column list shared by all queries returning InstalledPart.
const PART_COLUMNS: &str =
    "id, name, version, release, description, arch, license, url, installed_at, install_size, pkg_hash, install_scripts, assumed, install_reason, epoch";

/// Map a row (with PART_COLUMNS order) to InstalledPart.
fn row_to_installed_part(row: &rusqlite::Row) -> rusqlite::Result<InstalledPart> {
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
        install_reason: row.get::<_, String>(13)?,
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

    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if ret != 0 {
        return Err(WrightError::DatabaseError(
            "database is locked by another process".to_string(),
        ));
    }

    Ok(file)
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

impl Database {
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn db_path(&self) -> Option<&Path> {
        self.db_path.as_deref()
    }

    /// Perform a physical integrity check on the SQLite database.
    pub fn integrity_check(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("PRAGMA integrity_check")?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        // "ok" means no errors
        if rows.len() == 1 && rows[0] == "ok" {
            Ok(Vec::new())
        } else {
            Ok(rows)
        }
    }

    /// Get details of all shadowed file ownerships (forced overwrites).
    pub fn get_shadowed_conflicts(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.path, p1.name as original, p2.name as shadower 
             FROM shadowed_files s
             JOIN parts p1 ON s.original_owner_id = p1.id
             JOIN parts p2 ON s.shadowed_by_id = p2.id",
        )?;

        let rows = stmt
            .query_map([], |row| {
                Ok(format!(
                    "Path '{}' (owned by {}) is shadowed by {}",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    pub fn insert_part(&self, part: NewPart) -> Result<i64> {
        // If an assumed record exists for this part, replace it with the real one.
        let was_assumed: bool = self
            .conn
            .query_row(
                "SELECT assumed FROM parts WHERE name = ?1",
                params![part.name],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if was_assumed {
            self.conn
                .execute(
                    "DELETE FROM parts WHERE name = ?1 AND assumed = 1",
                    params![part.name],
                )
                .map_err(|e| {
                    WrightError::DatabaseError(format!("failed to remove assumed record: {}", e))
                })?;
        }

        self.conn
            .execute(
                "INSERT INTO parts (name, version, release, epoch, description, arch, license, url, install_size, pkg_hash, install_scripts, install_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    part.name,
                    part.version,
                    part.release,
                    part.epoch,
                    part.description,
                    part.arch,
                    part.license,
                    part.url,
                    part.install_size,
                    part.pkg_hash,
                    part.install_scripts,
                    part.install_reason
                ],
            )
            .map_err(|e| {
                if let rusqlite::Error::SqliteFailure(ref err, _) = e {
                    if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE {
                        return WrightError::PartAlreadyInstalled(part.name.to_string());
                    }
                }
                WrightError::DatabaseError(format!("failed to insert part: {}", e))
            })?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Register an externally-provided part so dependency checks treat it as satisfied.
    /// If the part is already assumed, updates its version. Idempotent.
    pub fn assume_part(&self, name: &str, version: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO parts (name, version, release, description, arch, license, install_size, assumed)
             VALUES (?1, ?2, 0, 'externally provided', 'any', 'unknown', 0, 1)
             ON CONFLICT(name) DO UPDATE SET version=excluded.version, assumed=1",
            params![name, version],
        ).map_err(|e| WrightError::DatabaseError(format!("failed to assume part: {}", e)))?;
        Ok(())
    }

    /// Remove an assumed part record. Returns an error if the part does not exist
    /// or is not assumed (i.e. was installed normally).
    pub fn unassume_part(&self, name: &str) -> Result<()> {
        let rows = self
            .conn
            .execute(
                "DELETE FROM parts WHERE name = ?1 AND assumed = 1",
                params![name],
            )
            .map_err(|e| WrightError::DatabaseError(format!("failed to unassume part: {}", e)))?;
        if rows == 0 {
            return Err(WrightError::PartNotFound(name.to_string()));
        }
        Ok(())
    }

    pub fn update_part(&self, part: NewPart) -> Result<()> {
        let rows = self.conn.execute(
            "UPDATE parts SET version = ?1, release = ?2, epoch = ?3, description = ?4, arch = ?5, license = ?6, url = ?7, install_size = ?8, pkg_hash = ?9, install_scripts = ?10
             WHERE name = ?11",
            params![
                part.version,
                part.release,
                part.epoch,
                part.description,
                part.arch,
                part.license,
                part.url,
                part.install_size,
                part.pkg_hash,
                part.install_scripts,
                part.name
            ],
        ).map_err(|e| WrightError::DatabaseError(format!("failed to update part: {}", e)))?;

        if rows == 0 {
            return Err(WrightError::PartNotFound(part.name.to_string()));
        }
        Ok(())
    }

    pub fn remove_part(&self, name: &str) -> Result<()> {
        let rows = self
            .conn
            .execute("DELETE FROM parts WHERE name = ?1", params![name])
            .map_err(|e| WrightError::DatabaseError(format!("failed to remove part: {}", e)))?;
        if rows == 0 {
            return Err(WrightError::PartNotFound(name.to_string()));
        }
        Ok(())
    }

    pub fn get_part(&self, name: &str) -> Result<Option<InstalledPart>> {
        let sql = format!("SELECT {} FROM parts WHERE name = ?1", PART_COLUMNS);
        let mut stmt = self.conn.prepare(&sql)?;

        match stmt.query_row(params![name], row_to_installed_part) {
            Ok(info) => Ok(Some(info)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(WrightError::DatabaseError(format!(
                "failed to query part: {}",
                e
            ))),
        }
    }

    pub fn list_parts(&self) -> Result<Vec<InstalledPart>> {
        let sql = format!("SELECT {} FROM parts ORDER BY name", PART_COLUMNS);
        let mut stmt = self.conn.prepare(&sql)?;

        let rows = stmt
            .query_map([], row_to_installed_part)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to list parts: {}", e)))?;

        Ok(rows)
    }

    /// Get parts that are not depended on by any other installed part.
    pub fn get_root_parts(&self) -> Result<Vec<InstalledPart>> {
        let sql = format!(
            "SELECT {} FROM parts WHERE name NOT IN (SELECT DISTINCT depends_on FROM dependencies) ORDER BY name",
            PART_COLUMNS
        );
        let mut stmt = self.conn.prepare(&sql)?;

        let rows = stmt
            .query_map([], row_to_installed_part)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to get root parts: {}", e)))?;

        Ok(rows)
    }

    pub fn search_parts(&self, keyword: &str) -> Result<Vec<InstalledPart>> {
        let pattern = format!("%{}%", keyword);
        let sql = format!(
            "SELECT {} FROM parts WHERE name LIKE ?1 OR description LIKE ?1 ORDER BY name",
            PART_COLUMNS
        );
        let mut stmt = self.conn.prepare(&sql)?;

        let rows = stmt
            .query_map(params![pattern], row_to_installed_part)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to search parts: {}", e)))?;

        Ok(rows)
    }

    pub fn record_shadowed_file(
        &self,
        path: &str,
        original_owner_id: i64,
        shadowed_by_id: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO shadowed_files (path, original_owner_id, shadowed_by_id) VALUES (?1, ?2, ?3)",
            params![path, original_owner_id, shadowed_by_id],
        )?;
        Ok(())
    }

    pub fn insert_files(&self, part_id: i64, files: &[FileEntry]) -> Result<()> {
        let tx = self.conn.unchecked_transaction().map_err(|e| {
            WrightError::DatabaseError(format!("failed to begin transaction: {}", e))
        })?;

        {
            let mut stmt = tx.prepare(
                "INSERT INTO files (part_id, path, file_hash, file_type, file_mode, file_size, is_config)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;

            for file in files {
                stmt.execute(params![
                    part_id,
                    file.path,
                    file.file_hash,
                    file.file_type,
                    file.file_mode,
                    file.file_size,
                    file.is_config,
                ])?;
            }
        }

        tx.commit()
            .map_err(|e| WrightError::DatabaseError(format!("failed to commit files: {}", e)))?;
        Ok(())
    }

    /// Find other parts that own the same path.
    pub fn get_other_owners(&self, current_pkg_id: i64, path: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.name FROM parts p JOIN files f ON p.id = f.part_id 
             WHERE f.path = ?1 AND p.id != ?2",
        )?;
        let rows = stmt
            .query_map(params![path, current_pkg_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn replace_files(&self, part_id: i64, files: &[FileEntry]) -> Result<()> {
        self.conn
            .execute("DELETE FROM files WHERE part_id = ?1", params![part_id])
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to delete old files: {}", e))
            })?;
        self.insert_files(part_id, files)
    }

    pub fn get_files(&self, part_id: i64) -> Result<Vec<FileEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, file_hash, file_type, file_mode, file_size, is_config
             FROM files WHERE part_id = ?1 ORDER BY path",
        )?;

        let rows = stmt
            .query_map(params![part_id], |row| {
                let ft_str: String = row.get(2)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    ft_str,
                    row.get::<_, Option<u32>>(3)?,
                    row.get::<_, Option<u64>>(4)?,
                    row.get::<_, bool>(5)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to get files: {}", e)))?;

        let rows = rows
            .into_iter()
            .map(
                |(path, file_hash, ft_str, file_mode, file_size, is_config)| {
                    let file_type = FileType::try_from(ft_str.as_str()).unwrap_or(FileType::File);
                    FileEntry {
                        path,
                        file_hash,
                        file_type,
                        file_mode,
                        file_size,
                        is_config,
                    }
                },
            )
            .collect();

        Ok(rows)
    }

    pub fn find_owner(&self, path: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.name FROM files f JOIN parts p ON f.part_id = p.id WHERE f.path = ?1",
        )?;

        match stmt.query_row(params![path], |row| row.get::<_, String>(0)) {
            Ok(name) => Ok(Some(name)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(WrightError::DatabaseError(format!(
                "failed to find owner: {}",
                e
            ))),
        }
    }

    pub fn insert_dependencies(&self, part_id: i64, deps: &[Dependency]) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "INSERT INTO dependencies (part_id, depends_on, version_constraint, dep_type)
             VALUES (?1, ?2, ?3, ?4)",
        )?;

        for dep in deps {
            stmt.execute(params![part_id, dep.name, dep.constraint, dep.dep_type])?;
        }

        Ok(())
    }

    pub fn replace_dependencies(&self, part_id: i64, deps: &[Dependency]) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM dependencies WHERE part_id = ?1",
                params![part_id],
            )
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to delete old dependencies: {}", e))
            })?;
        self.insert_dependencies(part_id, deps)
    }

    pub fn check_dependency(&self, name: &str) -> Result<bool> {
        let mut stmt = self
            .conn
            .prepare("SELECT COUNT(*) FROM parts WHERE name = ?1")?;
        let count: i64 = stmt.query_row(params![name], |row| row.get(0))?;
        if count > 0 {
            return Ok(true);
        }
        // Also check if any installed part provides this name
        let mut stmt2 = self
            .conn
            .prepare("SELECT COUNT(*) FROM provides WHERE name = ?1")?;
        let prov_count: i64 = stmt2.query_row(params![name], |row| row.get(0))?;
        Ok(prov_count > 0)
    }

    pub fn get_dependents(&self, name: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT p.name, d.dep_type FROM dependencies d
             JOIN parts p ON d.part_id = p.id
             WHERE d.depends_on = ?1",
        )?;

        let rows = stmt
            .query_map(params![name], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to get dependents: {}", e)))?;

        Ok(rows)
    }

    pub fn get_dependencies(&self, part_id: i64) -> Result<Vec<Dependency>> {
        let mut stmt = self.conn.prepare(
            "SELECT depends_on, version_constraint, dep_type FROM dependencies WHERE part_id = ?1",
        )?;

        let rows = stmt
            .query_map(params![part_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to get dependencies: {}", e))
            })?;

        Ok(rows
            .into_iter()
            .map(|(name, constraint, dt_str)| Dependency {
                name,
                constraint,
                dep_type: DepType::try_from(dt_str.as_str()).unwrap_or(DepType::Runtime),
            })
            .collect())
    }

    /// Get runtime dependencies for a part by name.
    pub fn get_dependencies_by_name(&self, name: &str) -> Result<Vec<Dependency>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.depends_on, d.version_constraint, d.dep_type
             FROM dependencies d
             JOIN parts p ON d.part_id = p.id
             WHERE p.name = ?1",
        )?;

        let rows = stmt
            .query_map(params![name], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to get dependencies: {}", e))
            })?;

        Ok(rows
            .into_iter()
            .map(|(n, constraint, dt_str)| Dependency {
                name: n,
                constraint,
                dep_type: DepType::try_from(dt_str.as_str()).unwrap_or(DepType::Runtime),
            })
            .collect())
    }

    /// Get all transitive reverse dependents of a part (parts that depend on it,
    /// directly or indirectly). Does NOT include the root part itself.
    /// Returns names in leaf-first order (safe removal order: remove leaves before parents).
    pub fn get_recursive_dependents(&self, name: &str) -> Result<Vec<String>> {
        let mut result = Vec::new();
        let mut visited = std::collections::HashSet::new();
        visited.insert(name.to_string()); // mark root as visited but don't add to result
        self.collect_dependents_recursive(name, &mut visited, &mut result)?;
        Ok(result)
    }

    fn collect_dependents_recursive(
        &self,
        name: &str,
        visited: &mut std::collections::HashSet<String>,
        result: &mut Vec<String>,
    ) -> Result<()> {
        let dependents = self.get_dependents(name)?;
        for (dep_name, _) in &dependents {
            if visited.contains(dep_name) {
                continue;
            }
            visited.insert(dep_name.to_string());
            // Recurse first so leaves are added before their parents
            self.collect_dependents_recursive(dep_name, visited, result)?;
            result.push(dep_name.to_string());
        }
        Ok(())
    }

    /// Update the install_reason of a part (e.g. promote dependency -> explicit).
    pub fn set_install_reason(&self, name: &str, reason: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE parts SET install_reason = ?1 WHERE name = ?2",
                params![reason, name],
            )
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to set install_reason: {}", e))
            })?;
        Ok(())
    }

    /// Get orphan dependencies of a specific part: dependencies that were auto-installed
    /// (`install_reason = 'dependency'`) and are not depended on by any other installed part.
    pub fn get_orphan_dependencies(&self, name: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.depends_on FROM dependencies d
             JOIN parts p ON d.part_id = p.id
             WHERE p.name = ?1
               AND EXISTS (
                   SELECT 1 FROM parts dep WHERE dep.name = d.depends_on AND dep.install_reason = 'dependency'
               )
               AND NOT EXISTS (
                   SELECT 1 FROM dependencies d2
                   JOIN parts p2 ON d2.part_id = p2.id
                   WHERE d2.depends_on = d.depends_on AND p2.name != ?1
               )"
        ).map_err(|e| WrightError::DatabaseError(format!("failed to prepare orphan deps query: {}", e)))?;

        let rows = stmt
            .query_map(params![name], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to get orphan deps: {}", e)))?;

        Ok(rows)
    }

    /// Get globally orphan parts: `install_reason = 'dependency'` and not depended on
    /// by any installed part.
    pub fn get_orphan_parts(&self) -> Result<Vec<InstalledPart>> {
        let sql = format!(
            "SELECT {} FROM parts WHERE install_reason = 'dependency' AND name NOT IN (
                SELECT depends_on FROM dependencies
            )",
            PART_COLUMNS
        );
        let mut stmt = self.conn.prepare(&sql).map_err(|e| {
            WrightError::DatabaseError(format!("failed to prepare orphan query: {}", e))
        })?;

        let rows = stmt
            .query_map([], row_to_installed_part)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to get orphan parts: {}", e))
            })?;

        Ok(rows)
    }

    pub fn record_transaction(
        &self,
        operation: &str,
        part_name: &str,
        old_version: Option<&str>,
        new_version: Option<&str>,
        status: &str,
        backup_path: Option<&str>,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO transactions (operation, part_name, old_version, new_version, status, backup_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![operation, part_name, old_version, new_version, status, backup_path],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Query transaction history, optionally filtered by part name.
    pub fn get_history(&self, part: Option<&str>) -> Result<Vec<TransactionRecord>> {
        let mut records = Vec::new();
        if let Some(name) = part {
            let mut stmt = self.conn.prepare(
                "SELECT timestamp, operation, part_name, old_version, new_version, status
                 FROM transactions WHERE part_name = ?1 ORDER BY timestamp",
            )?;
            let rows = stmt.query_map(params![name], row_to_transaction)?;
            for row in rows {
                records.push(row?);
            }
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT timestamp, operation, part_name, old_version, new_version, status
                 FROM transactions ORDER BY timestamp",
            )?;
            let rows = stmt.query_map([], row_to_transaction)?;
            for row in rows {
                records.push(row?);
            }
        }
        Ok(records)
    }

    pub fn update_transaction_status(&self, id: i64, status: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE transactions SET status = ?1 WHERE id = ?2",
            params![status, id],
        )?;
        Ok(())
    }

    pub fn insert_optional_dependencies(
        &self,
        part_id: i64,
        deps: &[(String, String)],
    ) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "INSERT INTO optional_dependencies (part_id, name, description) VALUES (?1, ?2, ?3)",
        )?;
        for (name, description) in deps {
            stmt.execute(params![part_id, name, description])?;
        }
        Ok(())
    }

    pub fn replace_optional_dependencies(
        &self,
        part_id: i64,
        deps: &[(String, String)],
    ) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM optional_dependencies WHERE part_id = ?1",
                params![part_id],
            )
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to delete old optional deps: {}", e))
            })?;
        self.insert_optional_dependencies(part_id, deps)
    }

    pub fn get_optional_dependencies(&self, part_id: i64) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, description FROM optional_dependencies WHERE part_id = ?1 ORDER BY name",
        )?;
        let rows = stmt
            .query_map(params![part_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to get optional deps: {}", e))
            })?;
        Ok(rows)
    }

    // ── provides ─────────────────────────────────────────────────────────

    pub fn insert_provides(&self, part_id: i64, names: &[String]) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare("INSERT INTO provides (part_id, name) VALUES (?1, ?2)")?;
        for name in names {
            stmt.execute(params![part_id, name])?;
        }
        Ok(())
    }

    pub fn get_provides(&self, part_id: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM provides WHERE part_id = ?1 ORDER BY name")?;
        let rows = stmt
            .query_map(params![part_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to get provides: {}", e)))?;
        Ok(rows)
    }

    /// Return names of all installed parts that provide `virtual_name`.
    pub fn find_providers(&self, virtual_name: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.name FROM parts p
             JOIN provides pv ON p.id = pv.part_id
             WHERE pv.name = ?1",
        )?;
        let rows = stmt
            .query_map(params![virtual_name], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to find providers: {}", e)))?;
        Ok(rows)
    }

    // ── conflicts ────────────────────────────────────────────────────────

    pub fn insert_conflicts(&self, part_id: i64, names: &[String]) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare("INSERT INTO conflicts (part_id, name) VALUES (?1, ?2)")?;
        for name in names {
            stmt.execute(params![part_id, name])?;
        }
        Ok(())
    }

    pub fn get_conflicts(&self, part_id: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM conflicts WHERE part_id = ?1 ORDER BY name")?;
        let rows = stmt
            .query_map(params![part_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to get conflicts: {}", e)))?;
        Ok(rows)
    }

    // ── build sessions ────────────────────────────────────────────────────

    /// Create a new build session, inserting all packages as `pending`.
    /// If a session with the same hash already exists, it is left intact
    /// (allows resuming).
    pub fn create_session(&self, session_hash: &str, packages: &[String]) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "INSERT OR IGNORE INTO build_sessions (session_hash, package_name, status) VALUES (?1, ?2, 'pending')",
        )?;
        for pkg in packages {
            stmt.execute(params![session_hash, pkg])?;
        }
        Ok(())
    }

    /// Mark a package as completed within a session.
    pub fn mark_session_completed(&self, session_hash: &str, package_name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE build_sessions SET status = 'completed' WHERE session_hash = ?1 AND package_name = ?2",
            params![session_hash, package_name],
        )?;
        Ok(())
    }

    /// Get the set of completed package names for a session.
    pub fn get_session_completed(&self, session_hash: &str) -> Result<HashSet<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT package_name FROM build_sessions WHERE session_hash = ?1 AND status = 'completed'",
        )?;
        let rows = stmt
            .query_map(params![session_hash], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<HashSet<_>, _>>()
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to query build session: {}", e))
            })?;
        Ok(rows)
    }

    /// Check whether a session with this hash exists (has any rows).
    pub fn session_exists(&self, session_hash: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM build_sessions WHERE session_hash = ?1",
            params![session_hash],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Remove all records for a session (called on successful completion).
    pub fn clear_session(&self, session_hash: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM build_sessions WHERE session_hash = ?1",
            params![session_hash],
        )?;
        Ok(())
    }

    /// Return names of installed parts whose `conflicts` list includes `name`.
    pub fn find_conflicting_parts(&self, name: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.name FROM parts p
             JOIN conflicts c ON p.id = c.part_id
             WHERE c.name = ?1",
        )?;
        let rows = stmt
            .query_map(params![name], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to find conflicting parts: {}", e))
            })?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        Database::open_in_memory().unwrap()
    }

    #[test]
    fn test_insert_and_get_package() {
        let db = test_db();
        let id = db
            .insert_part(NewPart {
                name: "hello",
                version: "1.0.0",
                release: 1,
                description: "test pkg",
                arch: "x86_64",
                license: "MIT",
                install_size: 1024,
                ..Default::default()
            })
            .unwrap();
        assert!(id > 0);

        let pkg = db.get_part("hello").unwrap().unwrap();
        assert_eq!(pkg.name, "hello");
        assert_eq!(pkg.version, "1.0.0");
        assert_eq!(pkg.release, 1);
        assert_eq!(pkg.install_size, 1024);
        assert!(pkg.install_scripts.is_none());
    }

    #[test]
    fn test_list_packages() {
        let db = test_db();
        db.insert_part(NewPart {
            name: "alpha",
            version: "1.0.0",
            release: 1,
            description: "a",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
        .unwrap();
        db.insert_part(NewPart {
            name: "beta",
            version: "2.0.0",
            release: 1,
            description: "b",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
        .unwrap();
        let pkgs = db.list_parts().unwrap();
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].name, "alpha");
        assert_eq!(pkgs[1].name, "beta");
    }

    #[test]
    fn test_remove_package() {
        let db = test_db();
        db.insert_part(NewPart {
            name: "hello",
            version: "1.0.0",
            release: 1,
            description: "test",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
        .unwrap();
        db.remove_part("hello").unwrap();
        assert!(db.get_part("hello").unwrap().is_none());
    }

    #[test]
    fn test_remove_cascades_files() {
        let db = test_db();
        let id = db
            .insert_part(NewPart {
                name: "hello",
                version: "1.0.0",
                release: 1,
                description: "test",
                arch: "x86_64",
                license: "MIT",
                ..Default::default()
            })
            .unwrap();
        db.insert_files(
            id,
            &[FileEntry {
                path: "/usr/bin/hello".to_string(),
                file_hash: Some("abc123".to_string()),
                file_type: FileType::File,
                file_mode: Some(0o755),
                file_size: Some(1024),
                is_config: false,
            }],
        )
        .unwrap();

        db.remove_part("hello").unwrap();
        assert!(db.find_owner("/usr/bin/hello").unwrap().is_none());
    }

    #[test]
    fn test_insert_and_get_files() {
        let db = test_db();
        let id = db
            .insert_part(NewPart {
                name: "hello",
                version: "1.0.0",
                release: 1,
                description: "test",
                arch: "x86_64",
                license: "MIT",
                ..Default::default()
            })
            .unwrap();

        let files = vec![
            FileEntry {
                path: "/usr/bin/hello".to_string(),
                file_hash: Some("abc".to_string()),
                file_type: FileType::File,
                file_mode: Some(0o755),
                file_size: Some(1024),
                is_config: false,
            },
            FileEntry {
                path: "/usr/share/hello/README".to_string(),
                file_hash: Some("def".to_string()),
                file_type: FileType::File,
                file_mode: Some(0o644),
                file_size: Some(512),
                is_config: false,
            },
        ];
        db.insert_files(id, &files).unwrap();

        let result = db.get_files(id).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].path, "/usr/bin/hello");
    }

    #[test]
    fn test_find_owner() {
        let db = test_db();
        let id = db
            .insert_part(NewPart {
                name: "hello",
                version: "1.0.0",
                release: 1,
                description: "test",
                arch: "x86_64",
                license: "MIT",
                ..Default::default()
            })
            .unwrap();
        db.insert_files(
            id,
            &[FileEntry {
                path: "/usr/bin/hello".to_string(),
                file_hash: None,
                file_type: FileType::File,
                file_mode: None,
                file_size: None,
                is_config: false,
            }],
        )
        .unwrap();

        assert_eq!(
            db.find_owner("/usr/bin/hello").unwrap(),
            Some("hello".to_string())
        );
        assert!(db.find_owner("/usr/bin/nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_search_packages() {
        let db = test_db();
        db.insert_part(NewPart {
            name: "hello",
            version: "1.0.0",
            release: 1,
            description: "Hello World",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
        .unwrap();
        db.insert_part(NewPart {
            name: "nginx",
            version: "1.25.3",
            release: 1,
            description: "HTTP server",
            arch: "x86_64",
            license: "BSD",
            ..Default::default()
        })
        .unwrap();

        let results = db.search_parts("hello").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "hello");

        let results = db.search_parts("server").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "nginx");
    }

    #[test]
    fn test_duplicate_package() {
        let db = test_db();
        db.insert_part(NewPart {
            name: "hello",
            version: "1.0.0",
            release: 1,
            description: "test",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
        .unwrap();
        let result = db.insert_part(NewPart {
            name: "hello",
            version: "2.0.0",
            release: 1,
            description: "test",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_check_dependency() {
        let db = test_db();
        db.insert_part(NewPart {
            name: "openssl",
            version: "3.0.0",
            release: 1,
            description: "SSL lib",
            arch: "x86_64",
            license: "Apache",
            ..Default::default()
        })
        .unwrap();
        assert!(db.check_dependency("openssl").unwrap());
        assert!(!db.check_dependency("nonexistent").unwrap());
    }

    #[test]
    fn test_record_transaction() {
        let db = test_db();
        let id = db
            .record_transaction("install", "hello", None, Some("1.0.0"), "completed", None)
            .unwrap();
        assert!(id > 0);
        db.update_transaction_status(id, "rolled_back").unwrap();
    }

    #[test]
    fn test_update_package() {
        let db = test_db();
        db.insert_part(NewPart {
            name: "hello",
            version: "1.0.0",
            release: 1,
            description: "test pkg",
            arch: "x86_64",
            license: "MIT",
            install_size: 1024,
            ..Default::default()
        })
        .unwrap();

        db.update_part(NewPart {
            name: "hello",
            version: "2.0.0",
            release: 1,
            description: "updated pkg",
            arch: "x86_64",
            license: "MIT",
            install_size: 2048,
            install_scripts: Some("post_install() { echo hi; }"),
            ..Default::default()
        })
        .unwrap();

        let pkg = db.get_part("hello").unwrap().unwrap();
        assert_eq!(pkg.version, "2.0.0");
        assert_eq!(pkg.description, "updated pkg");
        assert_eq!(pkg.install_size, 2048);
        assert_eq!(
            pkg.install_scripts.as_deref(),
            Some("post_install() { echo hi; }")
        );
    }

    #[test]
    fn test_replace_files() {
        let db = test_db();
        let id = db
            .insert_part(NewPart {
                name: "hello",
                version: "1.0.0",
                release: 1,
                description: "test",
                arch: "x86_64",
                license: "MIT",
                ..Default::default()
            })
            .unwrap();

        db.insert_files(
            id,
            &[FileEntry {
                path: "/usr/bin/hello".to_string(),
                file_hash: Some("abc".to_string()),
                file_type: FileType::File,
                file_mode: Some(0o755),
                file_size: Some(1024),
                is_config: false,
            }],
        )
        .unwrap();

        db.replace_files(
            id,
            &[FileEntry {
                path: "/usr/bin/hello2".to_string(),
                file_hash: Some("def".to_string()),
                file_type: FileType::File,
                file_mode: Some(0o755),
                file_size: Some(2048),
                is_config: false,
            }],
        )
        .unwrap();

        let files = db.get_files(id).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "/usr/bin/hello2");
    }

    #[test]
    fn test_replace_dependencies() {
        let db = test_db();
        let id = db
            .insert_part(NewPart {
                name: "hello",
                version: "1.0.0",
                release: 1,
                description: "test",
                arch: "x86_64",
                license: "MIT",
                ..Default::default()
            })
            .unwrap();

        db.insert_dependencies(
            id,
            &[Dependency {
                name: "openssl".to_string(),
                constraint: Some(">= 3.0".to_string()),
                dep_type: DepType::Runtime,
            }],
        )
        .unwrap();
        db.replace_dependencies(
            id,
            &[Dependency {
                name: "zlib".to_string(),
                constraint: None,
                dep_type: DepType::Runtime,
            }],
        )
        .unwrap();

        let deps = db.get_dependencies(id).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "zlib");
        assert_eq!(deps[0].dep_type, DepType::Runtime);
    }

    #[test]
    fn test_install_scripts_field() {
        let db = test_db();
        let id = db
            .insert_part(NewPart {
                name: "hello",
                version: "1.0.0",
                release: 1,
                description: "test",
                arch: "x86_64",
                license: "MIT",
                install_scripts: Some("post_install() { echo done; }"),
                ..Default::default()
            })
            .unwrap();

        let pkg = db.get_part("hello").unwrap().unwrap();
        assert_eq!(
            pkg.install_scripts.as_deref(),
            Some("post_install() { echo done; }")
        );

        let _ = id;
    }

    #[test]
    fn test_database_lock_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let _db1 = Database::open(&db_path).unwrap();
        let result = Database::open(&db_path);
        match result {
            Err(ref e) => {
                let err_msg = format!("{}", e);
                assert!(
                    err_msg.contains("locked"),
                    "Expected lock error, got: {}",
                    err_msg
                );
            }
            Ok(_) => panic!("Expected lock error, but open succeeded"),
        }
    }
}
