use crate::error::{Result, WrightError};

use super::{Database, FileEntry, FileType};

impl Database {
    pub fn record_shadowed_file(
        &self,
        path: &str,
        original_owner_id: i64,
        shadowed_by_id: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO shadowed_files (path, original_owner_id, shadowed_by_id) VALUES (?1, ?2, ?3)",
            rusqlite::params![path, original_owner_id, shadowed_by_id],
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
}
