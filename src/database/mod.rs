pub mod schema;

use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use crate::error::{WrightError, Result};

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
            _ => Err(WrightError::DatabaseError(format!("unknown file type: {}", s))),
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
            _ => Err(WrightError::DatabaseError(format!("unknown dep type: {}", s))),
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
pub struct PackageInfo {
    pub id: i64,
    pub name: String,
    pub version: String,
    pub release: u32,
    pub description: String,
    pub arch: String,
    pub license: String,
    pub url: Option<String>,
    pub installed_at: String,
    pub install_size: u64,
    pub pkg_hash: Option<String>,
    pub install_scripts: Option<String>,
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

#[derive(Debug, Clone, Default)]
pub struct NewPackage<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub release: u32,
    pub description: &'a str,
    pub arch: &'a str,
    pub license: &'a str,
    pub url: Option<&'a str>,
    pub install_size: u64,
    pub pkg_hash: Option<&'a str>,
    pub install_scripts: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: String,
    pub constraint: Option<String>,
    pub dep_type: DepType,
}

pub struct Database {
    conn: Connection,
    _lock_file: Option<File>,
    db_path: Option<PathBuf>,
}

/// Column list shared by all queries returning PackageInfo.
const PKG_COLUMNS: &str =
    "id, name, version, release, description, arch, license, url, installed_at, install_size, pkg_hash, install_scripts";

/// Map a row (with PKG_COLUMNS order) to PackageInfo.
fn row_to_package_info(row: &rusqlite::Row) -> rusqlite::Result<PackageInfo> {
    Ok(PackageInfo {
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
             JOIN packages p1 ON s.original_owner_id = p1.id
             JOIN packages p2 ON s.shadowed_by_id = p2.id"
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

    pub fn insert_package(&self, pkg: NewPackage) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO packages (name, version, release, description, arch, license, url, install_size, pkg_hash, install_scripts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    pkg.name,
                    pkg.version,
                    pkg.release,
                    pkg.description,
                    pkg.arch,
                    pkg.license,
                    pkg.url,
                    pkg.install_size,
                    pkg.pkg_hash,
                    pkg.install_scripts
                ],
            )
            .map_err(|e| {
                if let rusqlite::Error::SqliteFailure(ref err, _) = e {
                    if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE {
                        return WrightError::PackageAlreadyInstalled(pkg.name.to_string());
                    }
                }
                WrightError::DatabaseError(format!("failed to insert package: {}", e))
            })?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn update_package(&self, pkg: NewPackage) -> Result<()> {
        let rows = self.conn.execute(
            "UPDATE packages SET version = ?1, release = ?2, description = ?3, arch = ?4, license = ?5, url = ?6, install_size = ?7, pkg_hash = ?8, install_scripts = ?9
             WHERE name = ?10",
            params![
                pkg.version,
                pkg.release,
                pkg.description,
                pkg.arch,
                pkg.license,
                pkg.url,
                pkg.install_size,
                pkg.pkg_hash,
                pkg.install_scripts,
                pkg.name
            ],
        ).map_err(|e| WrightError::DatabaseError(format!("failed to update package: {}", e)))?;

        if rows == 0 {
            return Err(WrightError::PackageNotFound(pkg.name.to_string()));
        }
        Ok(())
    }

    pub fn remove_package(&self, name: &str) -> Result<()> {
        let rows = self
            .conn
            .execute("DELETE FROM packages WHERE name = ?1", params![name])
            .map_err(|e| WrightError::DatabaseError(format!("failed to remove package: {}", e)))?;
        if rows == 0 {
            return Err(WrightError::PackageNotFound(name.to_string()));
        }
        Ok(())
    }

    pub fn get_package(&self, name: &str) -> Result<Option<PackageInfo>> {
        let sql = format!("SELECT {} FROM packages WHERE name = ?1", PKG_COLUMNS);
        let mut stmt = self.conn.prepare(&sql)?;

        match stmt.query_row(params![name], row_to_package_info) {
            Ok(info) => Ok(Some(info)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(WrightError::DatabaseError(format!(
                "failed to query package: {}", e
            ))),
        }
    }

    pub fn list_packages(&self) -> Result<Vec<PackageInfo>> {
        let sql = format!("SELECT {} FROM packages ORDER BY name", PKG_COLUMNS);
        let mut stmt = self.conn.prepare(&sql)?;

        let rows = stmt
            .query_map([], row_to_package_info)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to list packages: {}", e)))?;

        Ok(rows)
    }

    /// Get packages that are not depended on by any other installed package.
    pub fn get_root_packages(&self) -> Result<Vec<PackageInfo>> {
        let sql = format!(
            "SELECT {} FROM packages WHERE name NOT IN (SELECT DISTINCT depends_on FROM dependencies) ORDER BY name",
            PKG_COLUMNS
        );
        let mut stmt = self.conn.prepare(&sql)?;

        let rows = stmt
            .query_map([], row_to_package_info)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to get root packages: {}", e)))?;

        Ok(rows)
    }

    pub fn search_packages(&self, keyword: &str) -> Result<Vec<PackageInfo>> {
        let pattern = format!("%{}%", keyword);
        let sql = format!(
            "SELECT {} FROM packages WHERE name LIKE ?1 OR description LIKE ?1 ORDER BY name",
            PKG_COLUMNS
        );
        let mut stmt = self.conn.prepare(&sql)?;

        let rows = stmt
            .query_map(params![pattern], row_to_package_info)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to search packages: {}", e)))?;

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

    pub fn insert_files(&self, package_id: i64, files: &[FileEntry]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()
            .map_err(|e| WrightError::DatabaseError(format!("failed to begin transaction: {}", e)))?;

        {
            let mut stmt = tx.prepare(
                "INSERT INTO files (package_id, path, file_hash, file_type, file_mode, file_size, is_config)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;

            for file in files {
                stmt.execute(params![
                    package_id,
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

    /// Find other packages that own the same path.
    pub fn get_other_owners(&self, current_pkg_id: i64, path: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.name FROM packages p JOIN files f ON p.id = f.package_id 
             WHERE f.path = ?1 AND p.id != ?2"
        )?;
        let rows = stmt
            .query_map(params![path, current_pkg_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn replace_files(&self, package_id: i64, files: &[FileEntry]) -> Result<()> {
        self.conn
            .execute("DELETE FROM files WHERE package_id = ?1", params![package_id])
            .map_err(|e| WrightError::DatabaseError(format!("failed to delete old files: {}", e)))?;
        self.insert_files(package_id, files)
    }

    pub fn get_files(&self, package_id: i64) -> Result<Vec<FileEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, file_hash, file_type, file_mode, file_size, is_config
             FROM files WHERE package_id = ?1 ORDER BY path",
        )?;

        let rows = stmt
            .query_map(params![package_id], |row| {
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

        let rows = rows.into_iter().map(|(path, file_hash, ft_str, file_mode, file_size, is_config)| {
            let file_type = FileType::try_from(ft_str.as_str())
                .unwrap_or(FileType::File);
            FileEntry { path, file_hash, file_type, file_mode, file_size, is_config }
        }).collect();

        Ok(rows)
    }

    pub fn find_owner(&self, path: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.name FROM files f JOIN packages p ON f.package_id = p.id WHERE f.path = ?1",
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

    pub fn insert_dependencies(
        &self,
        package_id: i64,
        deps: &[Dependency],
    ) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "INSERT INTO dependencies (package_id, depends_on, version_constraint, dep_type)
             VALUES (?1, ?2, ?3, ?4)",
        )?;

        for dep in deps {
            stmt.execute(params![package_id, dep.name, dep.constraint, dep.dep_type])?;
        }

        Ok(())
    }

    pub fn replace_dependencies(
        &self,
        package_id: i64,
        deps: &[Dependency],
    ) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM dependencies WHERE package_id = ?1",
                params![package_id],
            )
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to delete old dependencies: {}", e))
            })?;
        self.insert_dependencies(package_id, deps)
    }

    pub fn check_dependency(&self, name: &str) -> Result<bool> {
        let mut stmt = self.conn.prepare(
            "SELECT COUNT(*) FROM packages WHERE name = ?1",
        )?;
        let count: i64 = stmt.query_row(params![name], |row| row.get(0))?;
        Ok(count > 0)
    }

    pub fn get_dependents(&self, name: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT p.name, d.dep_type FROM dependencies d
             JOIN packages p ON d.package_id = p.id
             WHERE d.depends_on = ?1",
        )?;

        let rows = stmt
            .query_map(params![name], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to get dependents: {}", e))
            })?;

        Ok(rows)
    }

    pub fn get_dependencies(&self, package_id: i64) -> Result<Vec<Dependency>> {
        let mut stmt = self.conn.prepare(
            "SELECT depends_on, version_constraint, dep_type FROM dependencies WHERE package_id = ?1",
        )?;

        let rows = stmt
            .query_map(params![package_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, String>(2)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to get dependencies: {}", e)))?;

        Ok(rows.into_iter().map(|(name, constraint, dt_str)| Dependency {
            name,
            constraint,
            dep_type: DepType::try_from(dt_str.as_str()).unwrap_or(DepType::Runtime),
        }).collect())
    }

    /// Get runtime dependencies for a package by name.
    pub fn get_dependencies_by_name(&self, name: &str) -> Result<Vec<Dependency>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.depends_on, d.version_constraint, d.dep_type
             FROM dependencies d
             JOIN packages p ON d.package_id = p.id
             WHERE p.name = ?1",
        )?;

        let rows = stmt
            .query_map(params![name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, String>(2)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to get dependencies: {}", e)))?;

        Ok(rows.into_iter().map(|(n, constraint, dt_str)| Dependency {
            name: n,
            constraint,
            dep_type: DepType::try_from(dt_str.as_str()).unwrap_or(DepType::Runtime),
        }).collect())
    }

    /// Get all transitive reverse dependents of a package (packages that depend on it,
    /// directly or indirectly). Does NOT include the root package itself.
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

    pub fn record_transaction(
        &self,
        operation: &str,
        package_name: &str,
        old_version: Option<&str>,
        new_version: Option<&str>,
        status: &str,
        backup_path: Option<&str>,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO transactions (operation, package_name, old_version, new_version, status, backup_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![operation, package_name, old_version, new_version, status, backup_path],
        )?;
        Ok(self.conn.last_insert_rowid())
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
        package_id: i64,
        deps: &[(String, String)],
    ) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "INSERT INTO optional_dependencies (package_id, name, description) VALUES (?1, ?2, ?3)",
        )?;
        for (name, description) in deps {
            stmt.execute(params![package_id, name, description])?;
        }
        Ok(())
    }

    pub fn replace_optional_dependencies(
        &self,
        package_id: i64,
        deps: &[(String, String)],
    ) -> Result<()> {
        self.conn.execute(
            "DELETE FROM optional_dependencies WHERE package_id = ?1",
            params![package_id],
        ).map_err(|e| WrightError::DatabaseError(format!("failed to delete old optional deps: {}", e)))?;
        self.insert_optional_dependencies(package_id, deps)
    }

    pub fn get_optional_dependencies(&self, package_id: i64) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, description FROM optional_dependencies WHERE package_id = ?1 ORDER BY name",
        )?;
        let rows = stmt
            .query_map(params![package_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to get optional deps: {}", e)))?;
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
            .insert_package(NewPackage {
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

        let pkg = db.get_package("hello").unwrap().unwrap();
        assert_eq!(pkg.name, "hello");
        assert_eq!(pkg.version, "1.0.0");
        assert_eq!(pkg.release, 1);
        assert_eq!(pkg.install_size, 1024);
        assert!(pkg.install_scripts.is_none());
    }

    #[test]
    fn test_list_packages() {
        let db = test_db();
        db.insert_package(NewPackage {
            name: "alpha",
            version: "1.0.0",
            release: 1,
            description: "a",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
        .unwrap();
        db.insert_package(NewPackage {
            name: "beta",
            version: "2.0.0",
            release: 1,
            description: "b",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
        .unwrap();
        let pkgs = db.list_packages().unwrap();
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].name, "alpha");
        assert_eq!(pkgs[1].name, "beta");
    }

    #[test]
    fn test_remove_package() {
        let db = test_db();
        db.insert_package(NewPackage {
            name: "hello",
            version: "1.0.0",
            release: 1,
            description: "test",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
        .unwrap();
        db.remove_package("hello").unwrap();
        assert!(db.get_package("hello").unwrap().is_none());
    }

    #[test]
    fn test_remove_cascades_files() {
        let db = test_db();
        let id = db
            .insert_package(NewPackage {
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

        db.remove_package("hello").unwrap();
        assert!(db.find_owner("/usr/bin/hello").unwrap().is_none());
    }

    #[test]
    fn test_insert_and_get_files() {
        let db = test_db();
        let id = db
            .insert_package(NewPackage {
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
            .insert_package(NewPackage {
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
        db.insert_package(NewPackage {
            name: "hello",
            version: "1.0.0",
            release: 1,
            description: "Hello World",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
        .unwrap();
        db.insert_package(NewPackage {
            name: "nginx",
            version: "1.25.3",
            release: 1,
            description: "HTTP server",
            arch: "x86_64",
            license: "BSD",
            ..Default::default()
        })
        .unwrap();

        let results = db.search_packages("hello").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "hello");

        let results = db.search_packages("server").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "nginx");
    }

    #[test]
    fn test_duplicate_package() {
        let db = test_db();
        db.insert_package(NewPackage {
            name: "hello",
            version: "1.0.0",
            release: 1,
            description: "test",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
        .unwrap();
        let result =
            db.insert_package(NewPackage {
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
        db.insert_package(NewPackage {
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
        db.insert_package(NewPackage {
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

        db.update_package(NewPackage {
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

        let pkg = db.get_package("hello").unwrap().unwrap();
        assert_eq!(pkg.version, "2.0.0");
        assert_eq!(pkg.description, "updated pkg");
        assert_eq!(pkg.install_size, 2048);
        assert_eq!(pkg.install_scripts.as_deref(), Some("post_install() { echo hi; }"));
    }

    #[test]
    fn test_replace_files() {
        let db = test_db();
        let id = db
            .insert_package(NewPackage {
                name: "hello",
                version: "1.0.0",
                release: 1,
                description: "test",
                arch: "x86_64",
                license: "MIT",
                ..Default::default()
            })
            .unwrap();

        db.insert_files(id, &[FileEntry {
            path: "/usr/bin/hello".to_string(),
            file_hash: Some("abc".to_string()),
            file_type: FileType::File,
            file_mode: Some(0o755),
            file_size: Some(1024),
            is_config: false,
        }]).unwrap();

        db.replace_files(id, &[FileEntry {
            path: "/usr/bin/hello2".to_string(),
            file_hash: Some("def".to_string()),
            file_type: FileType::File,
            file_mode: Some(0o755),
            file_size: Some(2048),
            is_config: false,
        }]).unwrap();

        let files = db.get_files(id).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "/usr/bin/hello2");
    }

    #[test]
    fn test_replace_dependencies() {
        let db = test_db();
        let id = db
            .insert_package(NewPackage {
                name: "hello",
                version: "1.0.0",
                release: 1,
                description: "test",
                arch: "x86_64",
                license: "MIT",
                ..Default::default()
            })
            .unwrap();

        db.insert_dependencies(id, &[Dependency { name: "openssl".to_string(), constraint: Some(">= 3.0".to_string()), dep_type: DepType::Runtime }]).unwrap();
        db.replace_dependencies(id, &[Dependency { name: "zlib".to_string(), constraint: None, dep_type: DepType::Runtime }]).unwrap();

        let deps = db.get_dependencies(id).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "zlib");
        assert_eq!(deps[0].dep_type, DepType::Runtime);
    }

    #[test]
    fn test_install_scripts_field() {
        let db = test_db();
        let id = db
            .insert_package(NewPackage {
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

        let pkg = db.get_package("hello").unwrap().unwrap();
        assert_eq!(pkg.install_scripts.as_deref(), Some("post_install() { echo done; }"));

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
                assert!(err_msg.contains("locked"), "Expected lock error, got: {}", err_msg);
            }
            Ok(_) => panic!("Expected lock error, but open succeeded"),
        }
    }
}
