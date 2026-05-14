use super::{InstalledDb, NewPlan};
use crate::error::{Result, WrightError};
use crate::part::archive::PartInfo;
use sqlx::{query, query_as};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PlanRecord {
    pub id: i64,
    pub name: String,
    pub version: String,
    pub release: i64,
    pub epoch: i64,
    pub arch: String,
    pub registered_at: Option<String>,
}

impl InstalledDb {
    pub async fn insert_plan(&self, plan: NewPlan<'_>) -> Result<i64> {
        let res = query(
            "INSERT INTO plans (name, version, release, epoch, arch)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(plan.name)
        .bind(plan.version)
        .bind(plan.release as i64)
        .bind(plan.epoch as i64)
        .bind(plan.arch)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref db_err) = e
                && db_err.is_unique_violation()
            {
                return WrightError::DatabaseError(format!(
                    "plan '{}' already registered",
                    plan.name
                ));
            }
            WrightError::DatabaseError(format!("failed to insert plan: {}", e))
        })?;

        Ok(res.last_insert_rowid())
    }

    pub async fn get_plan(&self, name: &str) -> Result<Option<PlanRecord>> {
        query_as::<_, PlanRecord>(
            "SELECT id, name, version, release, epoch, arch, registered_at
             FROM plans WHERE name = ?",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to get plan: {}", e)))
    }

    pub async fn get_plan_by_id(&self, id: i64) -> Result<Option<PlanRecord>> {
        query_as::<_, PlanRecord>(
            "SELECT id, name, version, release, epoch, arch, registered_at
             FROM plans WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to get plan by id: {}", e)))
    }

    pub async fn list_plans(&self) -> Result<Vec<PlanRecord>> {
        query_as::<_, PlanRecord>(
            "SELECT id, name, version, release, epoch, arch, registered_at
             FROM plans ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to list plans: {}", e)))
    }

    pub async fn remove_plan(&self, name: &str) -> Result<()> {
        let res = query("DELETE FROM plans WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to remove plan: {}", e)))?;

        if res.rows_affected() == 0 {
            return Err(WrightError::DatabaseError(format!(
                "plan not found: {}",
                name
            )));
        }
        Ok(())
    }

    pub async fn get_parts_by_plan_id(&self, plan_id: i64) -> Result<Vec<super::InstalledPart>> {
        use super::PART_COLUMNS;
        let sql = format!(
            "SELECT {} FROM parts WHERE plan_id = ? ORDER BY name",
            PART_COLUMNS
        );
        query_as::<_, super::InstalledPart>(&sql)
            .bind(plan_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to get parts by plan_id: {}", e))
            })
    }

    pub async fn get_plan_id_by_name(&self, name: &str) -> Result<Option<i64>> {
        let row = query("SELECT id FROM plans WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to query plan id: {}", e)))?;

        match row {
            Some(r) => {
                use sqlx::Row;
                let id: i64 = r
                    .try_get(0)
                    .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
                Ok(Some(id))
            }
            None => Ok(None),
        }
    }

    /// Ensure a plan is registered in the database from pack metadata.
    /// If the plan already exists, updates its version metadata to match the pack.
    pub async fn ensure_plan_registered(
        &self,
        partinfo: &PartInfo,
        version: &str,
        release: u32,
        epoch: u32,
        arch: &str,
    ) -> Result<i64> {
        if let Some(existing) = self.get_plan(&partinfo.plan.name).await? {
            query("UPDATE plans SET version = ?, release = ?, epoch = ?, arch = ? WHERE id = ?")
                .bind(version)
                .bind(release as i64)
                .bind(epoch as i64)
                .bind(arch)
                .bind(existing.id)
                .execute(&self.pool)
                .await
                .map_err(|e| WrightError::DatabaseError(format!("failed to update plan: {}", e)))?;

            Ok(existing.id)
        } else {
            self.insert_plan(NewPlan {
                name: &partinfo.plan.name,
                version,
                release,
                epoch,
                arch,
            })
            .await
        }
    }
}
