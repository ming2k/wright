use super::{InstalledDb, InstalledPart, NewPart, Origin, PART_COLUMNS};
use crate::error::{Result, WrightError};
use sqlx::{query, query_as};

impl InstalledDb {
    pub async fn insert_part(&self, part: NewPart<'_>) -> Result<i64> {
        let row = query("SELECT assumed FROM parts WHERE name = ?")
            .bind(part.name)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to query assumed: {}", e)))?;

        let was_assumed = match row {
            Some(r) => {
                use sqlx::Row;
                let val: i64 = r
                    .try_get(0)
                    .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
                val != 0
            }
            None => false,
        };

        if was_assumed {
            query("DELETE FROM parts WHERE name = ? AND assumed = 1")
                .bind(part.name)
                .execute(&self.pool)
                .await
                .map_err(|e| {
                    WrightError::DatabaseError(format!("failed to remove assumed record: {}", e))
                })?;
        }

        let res = query(
            "INSERT INTO parts (name, version, release, epoch, description, arch, license, url, install_size, part_hash, install_scripts, origin, plan_name)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(part.name)
            .bind(part.version)
            .bind(part.release as i64)
            .bind(part.epoch as i64)
            .bind(part.description)
            .bind(part.arch)
            .bind(part.license)
            .bind(part.url)
            .bind(part.install_size as i64)
            .bind(part.part_hash)
            .bind(part.install_scripts)
            .bind(part.origin)
            .bind(part.plan_name)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref db_err) = e {
                if db_err.is_unique_violation() {
                    return WrightError::PartAlreadyInstalled(part.name.to_string());
                }
            }
            WrightError::DatabaseError(format!("failed to insert part: {}", e))
        })?;

        Ok(res.last_insert_rowid())
    }

    pub async fn assume_part(&self, name: &str, version: &str) -> Result<()> {
        query(
            "INSERT INTO parts (name, version, release, description, arch, license, install_size, assumed)
             VALUES (?, ?, 0, 'externally provided', 'any', 'unknown', 0, 1)
             ON CONFLICT(name) DO UPDATE SET version=excluded.version, assumed=1")
            .bind(name)
            .bind(version)
        .execute(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to assume part: {}", e)))?;
        Ok(())
    }

    pub async fn unassume_part(&self, name: &str) -> Result<()> {
        let res = query("DELETE FROM parts WHERE name = ? AND assumed = 1")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to unassume part: {}", e)))?;

        if res.rows_affected() == 0 {
            return Err(WrightError::PartNotFound(name.to_string()));
        }
        Ok(())
    }

    pub async fn update_part(&self, part: NewPart<'_>) -> Result<()> {
        let res = query(
            "UPDATE parts SET version = ?, release = ?, epoch = ?, description = ?, arch = ?, license = ?, url = ?, install_size = ?, part_hash = ?, install_scripts = ?, plan_name = ?
             WHERE name = ?")
            .bind(part.version)
            .bind(part.release as i64)
            .bind(part.epoch as i64)
            .bind(part.description)
            .bind(part.arch)
            .bind(part.license)
            .bind(part.url)
            .bind(part.install_size as i64)
            .bind(part.part_hash)
            .bind(part.install_scripts)
            .bind(part.plan_name)
            .bind(part.name)
        .execute(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to update part: {}", e)))?;

        if res.rows_affected() == 0 {
            return Err(WrightError::PartNotFound(part.name.to_string()));
        }
        Ok(())
    }

    pub async fn remove_part(&self, name: &str) -> Result<()> {
        let res = query("DELETE FROM parts WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to remove part: {}", e)))?;

        if res.rows_affected() == 0 {
            return Err(WrightError::PartNotFound(name.to_string()));
        }
        Ok(())
    }

    pub async fn get_part(&self, name: &str) -> Result<Option<InstalledPart>> {
        let sql = format!("SELECT {} FROM parts WHERE name = ?", PART_COLUMNS);
        query_as::<_, InstalledPart>(&sql)
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to query part: {}", e)))
    }

    pub async fn list_parts(&self) -> Result<Vec<InstalledPart>> {
        let sql = format!("SELECT {} FROM parts ORDER BY name", PART_COLUMNS);
        query_as::<_, InstalledPart>(&sql)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to list parts: {}", e)))
    }

    pub async fn get_root_parts(&self) -> Result<Vec<InstalledPart>> {
        let sql = format!(
            "SELECT {} FROM parts WHERE name NOT IN (SELECT DISTINCT depends_on FROM dependencies) ORDER BY name",
            PART_COLUMNS
        );
        query_as::<_, InstalledPart>(&sql)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to get root parts: {}", e)))
    }

    pub async fn search_parts(&self, keyword: &str) -> Result<Vec<InstalledPart>> {
        let pattern = format!("%{}%", keyword);
        let sql = format!(
            "SELECT {} FROM parts WHERE name LIKE ? OR description LIKE ? ORDER BY name",
            PART_COLUMNS
        );
        query_as::<_, InstalledPart>(&sql)
            .bind(&pattern)
            .bind(&pattern)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to search parts: {}", e)))
    }

    pub async fn set_origin(&self, name: &str, new_origin: Origin) -> Result<()> {
        if let Some(existing) = self.get_part(name).await? {
            if new_origin <= existing.origin {
                return Ok(());
            }
        }
        query("UPDATE parts SET origin = ? WHERE name = ?")
            .bind(new_origin)
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to set origin: {}", e)))?;
        Ok(())
    }

    pub async fn force_set_origin(&self, name: &str, new_origin: Origin) -> Result<()> {
        let res = query("UPDATE parts SET origin = ? WHERE name = ?")
            .bind(new_origin)
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to set origin: {}", e)))?;

        if res.rows_affected() == 0 {
            return Err(WrightError::PartNotFound(name.to_string()));
        }
        Ok(())
    }

    pub async fn get_orphan_parts(&self) -> Result<Vec<InstalledPart>> {
        let sql = format!(
            "SELECT {} FROM parts WHERE origin = 'dependency' AND name NOT IN (
                SELECT depends_on FROM dependencies
            )",
            PART_COLUMNS
        );
        query_as::<_, InstalledPart>(&sql)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to get orphan parts: {}", e)))
    }

    pub async fn get_assumed_parts(&self) -> Result<Vec<InstalledPart>> {
        let sql = format!("SELECT {} FROM parts WHERE assumed = 1 ORDER BY name", PART_COLUMNS);
        query_as::<_, InstalledPart>(&sql)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to get assumed parts: {}", e)))
    }

    pub async fn get_parts_by_plan(&self, plan_name: &str) -> Result<Vec<InstalledPart>> {
        let sql = format!("SELECT {} FROM parts WHERE plan_name = ? ORDER BY name", PART_COLUMNS);
        query_as::<_, InstalledPart>(&sql)
            .bind(plan_name)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to get parts by plan: {}", e)))
    }

    pub async fn remove_parts_by_plan(&self, plan_name: &str) -> Result<u64> {
        let res = query("DELETE FROM parts WHERE plan_name = ?")
            .bind(plan_name)
            .execute(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to remove parts by plan: {}", e)))?;
        Ok(res.rows_affected())
    }
}
