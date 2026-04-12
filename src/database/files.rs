use std::collections::HashMap;

use crate::error::{Result, WrightError};

use super::{Database, FileEntry, FileType};

impl Database {
    pub fn record_shadowed_file(
        &self,
        path: &str,
        original_owner_id: i64,
        shadowed_by_id: i64,
        diverted_to: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO shadowed_files (path, original_owner_id, shadowed_by_id, diverted_to) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![path, original_owner_id, shadowed_by_id, diverted_to],
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
                stmt.execute(rusqlite::params![
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

    pub fn get_other_owners(&self, current_pkg_id: i64, path: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.name FROM parts p JOIN files f ON p.id = f.part_id
              WHERE f.path = ?1 AND p.id != ?2",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![path, current_pkg_id], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_diverted_file(&self, path: &str, shadowed_by_id: i64) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT diverted_to FROM shadowed_files WHERE path = ?1 AND shadowed_by_id = ?2",
        )?;
        let mut rows = stmt.query(rusqlite::params![path, shadowed_by_id])?;
        if let Some(row) = rows.next()? {
            Ok(row.get(0)?)
        } else {
            Ok(None)
        }
    }

    pub fn get_all_diverted_files(&self, shadowed_by_id: i64) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, diverted_to FROM shadowed_files WHERE shadowed_by_id = ?1 AND diverted_to IS NOT NULL",
        )?;
        let rows = stmt.query_map(rusqlite::params![shadowed_by_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn remove_shadowed_records(&self, shadowed_by_id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM shadowed_files WHERE shadowed_by_id = ?1",
            rusqlite::params![shadowed_by_id],
        )?;
        Ok(())
    }
    pub fn replace_files(&self, part_id: i64, files: &[FileEntry]) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM files WHERE part_id = ?1",
                rusqlite::params![part_id],
            )
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
            .query_map(rusqlite::params![part_id], |row| {
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

        Ok(rows
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
            .collect())
    }

    pub fn find_owner(&self, path: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.name FROM files f JOIN parts p ON f.part_id = p.id WHERE f.path = ?1",
        )?;

        match stmt.query_row(rusqlite::params![path], |row| row.get::<_, String>(0)) {
            Ok(name) => Ok(Some(name)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(WrightError::DatabaseError(format!(
                "failed to find owner: {}",
                e
            ))),
        }
    }

    /// Batch version of find_owner. Returns a map of path -> owner name for all paths that have an owner.
    pub fn find_owners_batch(&self, paths: &[&str]) -> Result<HashMap<String, String>> {
        let mut result = HashMap::new();
        for chunk in paths.chunks(999) {
            let placeholders = chunk
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT f.path, p.name FROM files f JOIN parts p ON f.part_id = p.id WHERE f.path IN ({})",
                placeholders
            );
            let mut stmt = self.conn.prepare(&sql).map_err(|e| {
                WrightError::DatabaseError(format!("failed to prepare find_owners_batch: {}", e))
            })?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| {
                    WrightError::DatabaseError(format!("failed to query find_owners_batch: {}", e))
                })?;
            for row in rows {
                let (path, owner) = row.map_err(|e| {
                    WrightError::DatabaseError(format!(
                        "failed to read find_owners_batch row: {}",
                        e
                    ))
                })?;
                result.insert(path, owner);
            }
        }
        Ok(result)
    }

    /// Batch version of get_other_owners. Returns a map of path -> list of other owner names.
    pub fn get_other_owners_batch(
        &self,
        current_pkg_id: i64,
        paths: &[&str],
    ) -> Result<HashMap<String, Vec<String>>> {
        let mut result: HashMap<String, Vec<String>> = HashMap::new();
        for chunk in paths.chunks(999) {
            let placeholders = chunk
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 2))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT f.path, p.name FROM parts p JOIN files f ON p.id = f.part_id \
                 WHERE p.id != ?1 AND f.path IN ({})",
                placeholders
            );
            let mut stmt = self.conn.prepare(&sql).map_err(|e| {
                WrightError::DatabaseError(format!(
                    "failed to prepare get_other_owners_batch: {}",
                    e
                ))
            })?;
            let params: Vec<Box<dyn rusqlite::ToSql>> =
                std::iter::once(Box::new(current_pkg_id) as Box<dyn rusqlite::ToSql>)
                    .chain(
                        chunk
                            .iter()
                            .map(|p| Box::new(*p) as Box<dyn rusqlite::ToSql>),
                    )
                    .collect();
            let rows = stmt
                .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| {
                    WrightError::DatabaseError(format!(
                        "failed to query get_other_owners_batch: {}",
                        e
                    ))
                })?;
            for row in rows {
                let (path, owner) = row.map_err(|e| {
                    WrightError::DatabaseError(format!(
                        "failed to read get_other_owners_batch row: {}",
                        e
                    ))
                })?;
                result.entry(path).or_default().push(owner);
            }
        }
        Ok(result)
    }
}
