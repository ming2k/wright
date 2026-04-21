use crate::error::{Result, WrightError};

use super::{row_to_installed_part, InstalledDb, InstalledPart, NewPart, Origin, PART_COLUMNS};

impl InstalledDb {
    pub fn insert_part(&self, part: NewPart) -> Result<i64> {
        let was_assumed: bool = self
            .conn
            .query_row(
                "SELECT assumed FROM parts WHERE name = ?1",
                rusqlite::params![part.name],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if was_assumed {
            self.conn
                .execute(
                    "DELETE FROM parts WHERE name = ?1 AND assumed = 1",
                    rusqlite::params![part.name],
                )
                .map_err(|e| {
                    WrightError::DatabaseError(format!("failed to remove assumed record: {}", e))
                })?;
        }

        self.conn
            .execute(
                "INSERT INTO parts (name, version, release, epoch, description, arch, license, url, install_size, part_hash, install_scripts, origin)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                rusqlite::params![
                    part.name,
                    part.version,
                    part.release,
                    part.epoch,
                    part.description,
                    part.arch,
                    part.license,
                    part.url,
                    part.install_size,
                    part.part_hash,
                    part.install_scripts,
                    part.origin
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

    pub fn assume_part(&self, name: &str, version: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO parts (name, version, release, description, arch, license, install_size, assumed)
             VALUES (?1, ?2, 0, 'externally provided', 'any', 'unknown', 0, 1)
             ON CONFLICT(name) DO UPDATE SET version=excluded.version, assumed=1",
            rusqlite::params![name, version],
        ).map_err(|e| WrightError::DatabaseError(format!("failed to assume part: {}", e)))?;
        Ok(())
    }

    pub fn unassume_part(&self, name: &str) -> Result<()> {
        let rows = self
            .conn
            .execute(
                "DELETE FROM parts WHERE name = ?1 AND assumed = 1",
                rusqlite::params![name],
            )
            .map_err(|e| WrightError::DatabaseError(format!("failed to unassume part: {}", e)))?;
        if rows == 0 {
            return Err(WrightError::PartNotFound(name.to_string()));
        }
        Ok(())
    }

    pub fn update_part(&self, part: NewPart) -> Result<()> {
        let rows = self.conn.execute(
            "UPDATE parts SET version = ?1, release = ?2, epoch = ?3, description = ?4, arch = ?5, license = ?6, url = ?7, install_size = ?8, part_hash = ?9, install_scripts = ?10
             WHERE name = ?11",
            rusqlite::params![
                part.version,
                part.release,
                part.epoch,
                part.description,
                part.arch,
                part.license,
                part.url,
                part.install_size,
                part.part_hash,
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
            .execute("DELETE FROM parts WHERE name = ?1", rusqlite::params![name])
            .map_err(|e| WrightError::DatabaseError(format!("failed to remove part: {}", e)))?;
        if rows == 0 {
            return Err(WrightError::PartNotFound(name.to_string()));
        }
        Ok(())
    }

    pub fn get_part(&self, name: &str) -> Result<Option<InstalledPart>> {
        let sql = format!("SELECT {} FROM parts WHERE name = ?1", PART_COLUMNS);
        let mut stmt = self.conn.prepare(&sql)?;

        match stmt.query_row(rusqlite::params![name], row_to_installed_part) {
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
            .query_map(rusqlite::params![pattern], row_to_installed_part)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to search parts: {}", e)))?;

        Ok(rows)
    }

    pub fn set_origin(&self, name: &str, new_origin: Origin) -> Result<()> {
        if let Some(existing) = self.get_part(name)? {
            if new_origin <= existing.origin {
                return Ok(());
            }
        }
        self.conn
            .execute(
                "UPDATE parts SET origin = ?1 WHERE name = ?2",
                rusqlite::params![new_origin, name],
            )
            .map_err(|e| WrightError::DatabaseError(format!("failed to set origin: {}", e)))?;
        Ok(())
    }

    pub fn force_set_origin(&self, name: &str, new_origin: Origin) -> Result<()> {
        let rows = self
            .conn
            .execute(
                "UPDATE parts SET origin = ?1 WHERE name = ?2",
                rusqlite::params![new_origin, name],
            )
            .map_err(|e| WrightError::DatabaseError(format!("failed to set origin: {}", e)))?;
        if rows == 0 {
            return Err(WrightError::PartNotFound(name.to_string()));
        }
        Ok(())
    }

    pub fn get_orphan_parts(&self) -> Result<Vec<InstalledPart>> {
        let sql = format!(
            "SELECT {} FROM parts WHERE origin = 'dependency' AND name NOT IN (
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
}
