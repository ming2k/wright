pub mod schema;

use std::path::Path;

use rusqlite::{params, Connection};

use crate::error::{WrightError, Result};

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
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: String,
    pub file_hash: Option<String>,
    pub file_type: String,
    pub file_mode: Option<u32>,
    pub file_size: Option<u64>,
    pub is_config: bool,
}

pub struct Database {
    conn: Connection,
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
        let conn = Connection::open(path)?;
        schema::init_db(&conn)?;
        Ok(Database { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        schema::init_db(&conn)?;
        Ok(Database { conn })
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn insert_package(
        &self,
        name: &str,
        version: &str,
        release: u32,
        description: &str,
        arch: &str,
        license: &str,
        url: Option<&str>,
        install_size: u64,
        pkg_hash: Option<&str>,
    ) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO packages (name, version, release, description, arch, license, url, install_size, pkg_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![name, version, release, description, arch, license, url, install_size, pkg_hash],
            )
            .map_err(|e| {
                if let rusqlite::Error::SqliteFailure(ref err, _) = e {
                    if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE {
                        return WrightError::PackageAlreadyInstalled(name.to_string());
                    }
                }
                WrightError::DatabaseError(format!("failed to insert package: {}", e))
            })?;
        Ok(self.conn.last_insert_rowid())
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
        let mut stmt = self.conn.prepare(
            "SELECT id, name, version, release, description, arch, license, url, installed_at, install_size, pkg_hash
             FROM packages WHERE name = ?1",
        )?;

        let result = stmt.query_row(params![name], |row| {
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
            })
        });

        match result {
            Ok(info) => Ok(Some(info)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(WrightError::DatabaseError(format!(
                "failed to query package: {}",
                e
            ))),
        }
    }

    pub fn list_packages(&self) -> Result<Vec<PackageInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, version, release, description, arch, license, url, installed_at, install_size, pkg_hash
             FROM packages ORDER BY name",
        )?;

        let rows = stmt
            .query_map([], |row| {
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
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to list packages: {}", e)))?;

        Ok(rows)
    }

    pub fn search_packages(&self, keyword: &str) -> Result<Vec<PackageInfo>> {
        let pattern = format!("%{}%", keyword);
        let mut stmt = self.conn.prepare(
            "SELECT id, name, version, release, description, arch, license, url, installed_at, install_size, pkg_hash
             FROM packages WHERE name LIKE ?1 OR description LIKE ?1 ORDER BY name",
        )?;

        let rows = stmt
            .query_map(params![pattern], |row| {
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
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to search packages: {}", e)))?;

        Ok(rows)
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

    pub fn get_files(&self, package_id: i64) -> Result<Vec<FileEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, file_hash, file_type, file_mode, file_size, is_config
             FROM files WHERE package_id = ?1 ORDER BY path",
        )?;

        let rows = stmt
            .query_map(params![package_id], |row| {
                Ok(FileEntry {
                    path: row.get(0)?,
                    file_hash: row.get(1)?,
                    file_type: row.get(2)?,
                    file_mode: row.get(3)?,
                    file_size: row.get(4)?,
                    is_config: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to get files: {}", e)))?;

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
        deps: &[(String, Option<String>)],
    ) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "INSERT INTO dependencies (package_id, depends_on, version_constraint)
             VALUES (?1, ?2, ?3)",
        )?;

        for (dep_name, constraint) in deps {
            stmt.execute(params![package_id, dep_name, constraint])?;
        }

        Ok(())
    }

    pub fn check_dependency(&self, name: &str) -> Result<bool> {
        let mut stmt = self.conn.prepare(
            "SELECT COUNT(*) FROM packages WHERE name = ?1",
        )?;
        let count: i64 = stmt.query_row(params![name], |row| row.get(0))?;
        Ok(count > 0)
    }

    pub fn get_dependents(&self, name: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT p.name FROM dependencies d
             JOIN packages p ON d.package_id = p.id
             WHERE d.depends_on = ?1",
        )?;

        let rows = stmt
            .query_map(params![name], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to get dependents: {}", e))
            })?;

        Ok(rows)
    }

    pub fn get_dependencies(&self, package_id: i64) -> Result<Vec<(String, Option<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT depends_on, version_constraint FROM dependencies WHERE package_id = ?1",
        )?;

        let rows = stmt
            .query_map(params![package_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to get dependencies: {}", e)))?;

        Ok(rows)
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
            .insert_package("hello", "1.0.0", 1, "test pkg", "x86_64", "MIT", None, 1024, None)
            .unwrap();
        assert!(id > 0);

        let pkg = db.get_package("hello").unwrap().unwrap();
        assert_eq!(pkg.name, "hello");
        assert_eq!(pkg.version, "1.0.0");
        assert_eq!(pkg.release, 1);
        assert_eq!(pkg.install_size, 1024);
    }

    #[test]
    fn test_list_packages() {
        let db = test_db();
        db.insert_package("alpha", "1.0.0", 1, "a", "x86_64", "MIT", None, 0, None)
            .unwrap();
        db.insert_package("beta", "2.0.0", 1, "b", "x86_64", "MIT", None, 0, None)
            .unwrap();
        let pkgs = db.list_packages().unwrap();
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].name, "alpha");
        assert_eq!(pkgs[1].name, "beta");
    }

    #[test]
    fn test_remove_package() {
        let db = test_db();
        db.insert_package("hello", "1.0.0", 1, "test", "x86_64", "MIT", None, 0, None)
            .unwrap();
        db.remove_package("hello").unwrap();
        assert!(db.get_package("hello").unwrap().is_none());
    }

    #[test]
    fn test_remove_cascades_files() {
        let db = test_db();
        let id = db
            .insert_package("hello", "1.0.0", 1, "test", "x86_64", "MIT", None, 0, None)
            .unwrap();
        db.insert_files(
            id,
            &[FileEntry {
                path: "/usr/bin/hello".to_string(),
                file_hash: Some("abc123".to_string()),
                file_type: "file".to_string(),
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
            .insert_package("hello", "1.0.0", 1, "test", "x86_64", "MIT", None, 0, None)
            .unwrap();

        let files = vec![
            FileEntry {
                path: "/usr/bin/hello".to_string(),
                file_hash: Some("abc".to_string()),
                file_type: "file".to_string(),
                file_mode: Some(0o755),
                file_size: Some(1024),
                is_config: false,
            },
            FileEntry {
                path: "/usr/share/hello/README".to_string(),
                file_hash: Some("def".to_string()),
                file_type: "file".to_string(),
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
            .insert_package("hello", "1.0.0", 1, "test", "x86_64", "MIT", None, 0, None)
            .unwrap();
        db.insert_files(
            id,
            &[FileEntry {
                path: "/usr/bin/hello".to_string(),
                file_hash: None,
                file_type: "file".to_string(),
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
        db.insert_package("hello", "1.0.0", 1, "Hello World", "x86_64", "MIT", None, 0, None)
            .unwrap();
        db.insert_package("nginx", "1.25.3", 1, "HTTP server", "x86_64", "BSD", None, 0, None)
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
        db.insert_package("hello", "1.0.0", 1, "test", "x86_64", "MIT", None, 0, None)
            .unwrap();
        let result =
            db.insert_package("hello", "2.0.0", 1, "test", "x86_64", "MIT", None, 0, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_check_dependency() {
        let db = test_db();
        db.insert_package("openssl", "3.0.0", 1, "SSL lib", "x86_64", "Apache", None, 0, None)
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
}
