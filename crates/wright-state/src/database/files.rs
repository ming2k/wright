//! Installed-file ownership and diversion queries.

use super::{FileEntry, InstalledDb, ReadOnlyDb};
use crate::error::{Result, WrightError};
use rusqlite::{TransactionBehavior, params};
use std::collections::HashMap;

impl ReadOnlyDb {
    pub async fn get_other_owners(&self, current_part_id: i64, path: &str) -> Result<Vec<String>> {
        let path = path.to_string();
        self.read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT p.name FROM parts p JOIN files f ON p.id = f.part_id WHERE f.path = ?1 AND p.id != ?2",
            )?;
            let rows = stmt.query_map(params![path, current_part_id], |row| row.get(0))?;
            let mut result = Vec::new();
            for r in rows {
                result.push(r?);
            }
            Ok(result)
        })
        .await
    }

    pub async fn get_diverted_file(
        &self,
        path: &str,
        shadowed_by_id: i64,
    ) -> Result<Option<String>> {
        let path = path.to_string();
        self.read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT diverted_to FROM shadowed_files WHERE path = ?1 AND shadowed_by_id = ?2",
            )?;
            let mut rows = stmt.query(params![path, shadowed_by_id])?;
            if let Some(row) = rows.next()? {
                Ok(row.get(0)?)
            } else {
                Ok(None)
            }
        })
        .await
    }

    pub async fn get_all_diverted_files(
        &self,
        shadowed_by_id: i64,
    ) -> Result<Vec<(String, String)>> {
        self.read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT path, diverted_to FROM shadowed_files WHERE shadowed_by_id = ?1 AND diverted_to IS NOT NULL",
            )?;
            let rows = stmt.query_map(params![shadowed_by_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut result = Vec::new();
            for r in rows {
                result.push(r?);
            }
            Ok(result)
        })
        .await
    }

    pub async fn get_files(&self, part_id: i64) -> Result<Vec<FileEntry>> {
        self.read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT path, file_hash, file_type, file_mode, file_size, is_config
                 FROM files WHERE part_id = ?1 ORDER BY path",
            )?;
            let rows = stmt.query_map(params![part_id], FileEntry::from_row)?;
            let mut result = Vec::new();
            for r in rows {
                result.push(r?);
            }
            Ok(result)
        })
        .await
    }

    pub async fn find_all_owners(&self, path: &str) -> Result<Vec<String>> {
        let path = path.to_string();
        self.read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT p.name FROM parts p
                 JOIN files f ON p.id = f.part_id
                 WHERE f.path = ?1
                 ORDER BY p.name",
            )?;
            let rows = stmt.query_map(params![path], |r| r.get(0))?;
            let mut result = Vec::new();
            for r in rows {
                result.push(r?);
            }
            Ok(result)
        })
        .await
    }

    pub async fn find_owners_batch(&self, paths: &[&str]) -> Result<HashMap<String, String>> {
        let paths_vec: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
        self.read(move |conn| {
            let mut result = HashMap::new();
            for chunk in paths_vec.chunks(500) {
                let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                let query = format!(
                    "SELECT f.path, p.name FROM files f JOIN parts p ON f.part_id = p.id WHERE f.path IN ({})",
                    placeholders
                );
                let mut stmt = conn.prepare(&query)?;
                let params = rusqlite::params_from_iter(chunk.iter());
                let rows = stmt.query_map(params, |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                for r in rows {
                    let (path, name) = r?;
                    result.insert(path, name);
                }
            }
            Ok(result)
        })
        .await
    }

    pub async fn get_other_owners_batch(
        &self,
        current_part_id: i64,
        paths: &[&str],
    ) -> Result<HashMap<String, Vec<String>>> {
        let paths_vec: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
        self.read(move |conn| {
            let mut result: HashMap<String, Vec<String>> = HashMap::new();
            for chunk in paths_vec.chunks(500) {
                let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                let query = format!(
                    "SELECT f.path, p.name FROM parts p JOIN files f ON p.id = f.part_id WHERE p.id != ?1 AND f.path IN ({})",
                    placeholders
                );
                let mut stmt = conn.prepare(&query)?;
                let mut all_params: Vec<&dyn rusqlite::types::ToSql> = Vec::with_capacity(chunk.len() + 1);
                all_params.push(&current_part_id);
                for p in chunk {
                    all_params.push(p);
                }
                let rows = stmt.query_map(rusqlite::params_from_iter(all_params), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                for r in rows {
                    let (path, name) = r?;
                    result.entry(path).or_default().push(name);
                }
            }
            Ok(result)
        })
        .await
    }

    /// Total deployed footprint recorded in the registry: sum of recorded
    /// file sizes and the file count. This is the "computed on demand" figure
    /// migration V6 promised when it dropped `parts.install_size`.
    pub async fn file_usage(&self) -> Result<(u64, u64)> {
        self.read(|conn| {
            let (bytes, files): (i64, i64) = conn.query_row(
                "SELECT COALESCE(SUM(file_size), 0), COUNT(*) FROM files",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            Ok((bytes.max(0) as u64, files.max(0) as u64))
        })
        .await
    }

    /// Every filesystem path the registry claims to own, across all parts.
    /// Used by `wright doctor --drift` to subtract the managed set from a live-root walk.
    pub async fn all_owned_paths(&self) -> Result<std::collections::HashSet<String>> {
        self.read(|conn| {
            let mut stmt = conn.prepare("SELECT path FROM files")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let mut result = std::collections::HashSet::new();
            for r in rows {
                result.insert(r?);
            }
            Ok(result)
        })
        .await
    }

    pub async fn get_file_ownership_conflicts(&self) -> Result<Vec<String>> {
        self.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT path, GROUP_CONCAT(p.name, ', ') as owners
                 FROM files f
                 JOIN parts p ON f.part_id = p.id
                 GROUP BY path
                 HAVING COUNT(DISTINCT part_id) > 1",
            )?;
            let rows = stmt.query_map([], |row| {
                let path: String = row.get(0)?;
                let owners: String = row.get(1)?;
                Ok(format!(
                    "Path '{}' is claimed by multiple parts: {}",
                    path, owners
                ))
            })?;
            let mut result = Vec::new();
            for r in rows {
                result.push(r?);
            }
            Ok(result)
        })
        .await
    }
}

impl InstalledDb {
    pub async fn record_shadowed_file(
        &self,
        path: &str,
        original_owner_id: i64,
        shadowed_by_id: i64,
        diverted_to: Option<&str>,
    ) -> Result<()> {
        let path = path.to_string();
        let diverted_to = diverted_to.map(|s| s.to_string());
        self.write(move |conn| {
            conn.execute(
                "INSERT INTO shadowed_files (path, original_owner_id, shadowed_by_id, diverted_to) VALUES (?1, ?2, ?3, ?4)",
                params![path, original_owner_id, shadowed_by_id, diverted_to],
            )
            .map_err(|e| WrightError::context("failed to record shadowed file", e))?;
            Ok(())
        })
        .await
    }

    pub async fn insert_files(&self, part_id: i64, files: &[FileEntry]) -> Result<()> {
        let files = files.to_vec();
        self.write(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| WrightError::context("failed to begin transaction", e))?;

            {
                let mut stmt = tx
                    .prepare(
                        "INSERT INTO files (part_id, path, file_hash, file_type, file_mode, file_size, is_config)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    )
                    .map_err(|e| WrightError::context("failed to prepare insert file statement", e))?;

                for f in &files {
                    stmt.execute(params![
                        part_id,
                        f.path,
                        f.file_hash,
                        f.file_type,
                        f.file_mode,
                        f.file_size,
                        f.is_config,
                    ])
                    .map_err(|e| WrightError::context("failed to insert file", e))?;
                }
            }

            tx.commit()
                .map_err(|e| WrightError::context("failed to commit files", e))?;
            Ok(())
        })
        .await
    }

    pub async fn remove_shadowed_records(&self, shadowed_by_id: i64) -> Result<()> {
        self.write(move |conn| {
            conn.execute(
                "DELETE FROM shadowed_files WHERE shadowed_by_id = ?1",
                params![shadowed_by_id],
            )
            .map_err(|e| WrightError::context("failed to remove shadowed records", e))?;
            Ok(())
        })
        .await
    }

    pub async fn replace_files(&self, part_id: i64, files: &[FileEntry]) -> Result<()> {
        let files = files.to_vec();
        self.write(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| WrightError::context("failed to begin replace transaction", e))?;

            tx.execute("DELETE FROM files WHERE part_id = ?1", params![part_id])
                .map_err(|e| WrightError::context("failed to delete old files", e))?;

            {
                let mut stmt = tx
                    .prepare(
                        "INSERT INTO files (part_id, path, file_hash, file_type, file_mode, file_size, is_config)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    )
                    .map_err(|e| WrightError::context("failed to prepare insert file statement", e))?;

                for f in &files {
                    stmt.execute(params![
                        part_id,
                        f.path,
                        f.file_hash,
                        f.file_type,
                        f.file_mode,
                        f.file_size,
                        f.is_config,
                    ])
                    .map_err(|e| WrightError::context("failed to insert file", e))?;
                }
            }

            tx.commit()
                .map_err(|e| WrightError::context("failed to commit replaced files", e))?;
            Ok(())
        })
        .await
    }
}
