use super::{InstalledDb, InstalledPart, NewPart, Origin, PartWithPlan, PART_COLUMNS};
use sqlx::Row;

const PART_WITH_PLAN_SQL: &str = "
    SELECT
        p.id, p.name, p.plan_id, p.installed_at, p.part_hash, p.install_scripts, p.origin,
        pl.name as plan_name, pl.version, pl.release, pl.epoch, pl.arch
    FROM parts p
    INNER JOIN plans pl ON p.plan_id = pl.id
";
use crate::error::{Result, WrightError};
use sqlx::{query, query_as};

impl InstalledDb {
    pub async fn insert_part(&self, part: NewPart<'_>) -> Result<i64> {
        // Remove any external placeholder with this name before inserting the real record.
        // External parts share their plan name with the real part, so without this the
        // UNIQUE(plan_id, name) constraint would fire.
        query("DELETE FROM parts WHERE name = ? AND origin = 'external'")
            .bind(part.name)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to clear external placeholder: {}", e))
            })?;

        let res = query(
            "INSERT INTO parts (name, plan_id, part_hash, install_scripts, origin)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(part.name)
        .bind(part.plan_id)
        .bind(part.part_hash)
        .bind(part.install_scripts)
        .bind(part.origin)
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
        // Refuse to overwrite a genuinely installed part.
        if let Some(existing) = self.get_part(name).await? {
            if existing.origin != Origin::External {
                return Err(WrightError::PartAlreadyInstalled(format!(
                    "{} is already installed; uninstall it before assuming",
                    name
                )));
            }
        }

        let plan_id = match self.get_plan_id_by_name(name).await? {
            Some(id) => {
                query("UPDATE plans SET version = ? WHERE id = ?")
                    .bind(version)
                    .bind(id)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| {
                        WrightError::DatabaseError(format!("failed to update plan: {}", e))
                    })?;
                id
            }
            None => query(
                "INSERT INTO plans (name, version, release, epoch, description, arch, license)
                     VALUES (?, ?, 0, 0, 'externally provided', 'any', 'unknown')",
            )
            .bind(name)
            .bind(version)
            .execute(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to insert plan: {}", e)))?
            .last_insert_rowid(),
        };

        query(
            "INSERT INTO parts (name, plan_id, part_hash, install_scripts, origin)
             VALUES (?, ?, NULL, NULL, 'external')
             ON CONFLICT(plan_id, name) DO UPDATE SET origin = 'external', plan_id = excluded.plan_id",
        )
        .bind(name)
        .bind(plan_id)
        .execute(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to register external part: {}", e)))?;
        Ok(())
    }

    pub async fn unassume_part(&self, name: &str) -> Result<()> {
        let res = query("DELETE FROM parts WHERE name = ? AND origin = 'external'")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to unassume part: {}", e)))?;

        if res.rows_affected() == 0 {
            return Err(WrightError::PartNotFound(name.to_string()));
        }

        // Clean up the plan record if no other parts reference it.
        if let Some(plan) = self.get_plan(name).await? {
            let count: i64 = query("SELECT COUNT(*) FROM parts WHERE plan_id = ?")
                .bind(plan.id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?
                .try_get(0)
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            if count == 0 {
                let _ = self.remove_plan(name).await;
            }
        }
        Ok(())
    }

    pub async fn update_part(&self, part: NewPart<'_>) -> Result<()> {
        let res = query(
            "UPDATE parts SET plan_id = ?, part_hash = ?, install_scripts = ?, origin = ?
             WHERE name = ?",
        )
        .bind(part.plan_id)
        .bind(part.part_hash)
        .bind(part.install_scripts)
        .bind(part.origin)
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

    pub async fn list_parts(&self) -> Result<Vec<PartWithPlan>> {
        let sql = format!("{} ORDER BY p.name", PART_WITH_PLAN_SQL);
        query_as::<_, PartWithPlan>(&sql)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to list parts: {}", e)))
    }

    pub async fn get_root_parts(&self) -> Result<Vec<PartWithPlan>> {
        let sql = format!(
            "{} WHERE p.name NOT IN (SELECT DISTINCT depends_on FROM dependencies) ORDER BY p.name",
            PART_WITH_PLAN_SQL
        );
        query_as::<_, PartWithPlan>(&sql)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to get root parts: {}", e)))
    }

    pub async fn set_origin(&self, name: &str, new_origin: Origin) -> Result<()> {
        if let Some(existing) = self.get_part(name).await? {
            // External parts are managed exclusively via assume/unassume.
            if existing.origin == Origin::External {
                return Ok(());
            }
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

    pub async fn get_orphan_parts(&self) -> Result<Vec<PartWithPlan>> {
        let sql = format!(
            "{} WHERE p.origin = 'dependency' AND p.name NOT IN (
                SELECT depends_on FROM dependencies
            )",
            PART_WITH_PLAN_SQL
        );
        query_as::<_, PartWithPlan>(&sql)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to get orphan parts: {}", e)))
    }

    pub async fn get_assumed_parts(&self) -> Result<Vec<PartWithPlan>> {
        let sql = format!(
            "{} WHERE p.origin = 'external' ORDER BY p.name",
            PART_WITH_PLAN_SQL
        );
        query_as::<_, PartWithPlan>(&sql)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to get assumed parts: {}", e)))
    }

    pub async fn get_parts_by_plan(&self, plan_name: &str) -> Result<Vec<PartWithPlan>> {
        let sql = format!("{} WHERE pl.name = ? ORDER BY p.name", PART_WITH_PLAN_SQL);
        query_as::<_, PartWithPlan>(&sql)
            .bind(plan_name)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to get parts by plan: {}", e)))
    }

    pub async fn remove_parts_by_plan(&self, plan_name: &str) -> Result<u64> {
        // Count parts before deleting the plan (cascade will remove them)
        let count_row = query(
            "SELECT COUNT(*) FROM parts INNER JOIN plans ON parts.plan_id = plans.id WHERE plans.name = ?"
        )
        .bind(plan_name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to count parts by plan: {}", e)))?;

        use sqlx::Row;
        let count: i64 = count_row
            .try_get(0)
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?;

        query("DELETE FROM plans WHERE name = ?")
            .bind(plan_name)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to remove parts by plan: {}", e))
            })?;

        Ok(count as u64)
    }
}
