use super::{InstalledDb, PART_COLUMNS};
use crate::error::{Result, WrightError};
use sqlx::{query, query_as};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PlanRecord {
    pub id: i64,
    pub name: String,
    pub build_deps: Option<String>,
    pub link_deps: Option<String>,
    pub created_at: Option<String>,
}

impl InstalledDb {
    pub async fn get_or_create_plan(
        &self,
        name: &str,
        build_deps: Option<&str>,
        link_deps: Option<&str>,
    ) -> Result<i64> {
        // Try to find existing plan
        if let Some(row) = query("SELECT id FROM plans WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to query plan: {}", e)))?
        {
            use sqlx::Row;
            let id: i64 = row
                .try_get(0)
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            
            // Update deps if provided
            if build_deps.is_some() || link_deps.is_some() {
                query("UPDATE plans SET build_deps = COALESCE(?, build_deps), link_deps = COALESCE(?, link_deps) WHERE id = ?")
                    .bind(build_deps)
                    .bind(link_deps)
                    .bind(id)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| WrightError::DatabaseError(format!("failed to update plan: {}", e)))?;
            }
            
            return Ok(id);
        }

        // Create new plan
        let res = query(
            "INSERT INTO plans (name, build_deps, link_deps) VALUES (?, ?, ?)")
            .bind(name)
            .bind(build_deps)
            .bind(link_deps)
        .execute(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to insert plan: {}", e)))?;

        Ok(res.last_insert_rowid())
    }

    pub async fn get_plan_by_name(&self, name: &str) -> Result<Option<PlanRecord>> {
        query_as::<_, PlanRecord>("SELECT id, name, build_deps, link_deps, created_at FROM plans WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to get plan: {}", e)))
    }

    pub async fn get_plan_by_id(&self, id: i64) -> Result<Option<PlanRecord>> {
        query_as::<_, PlanRecord>("SELECT id, name, build_deps, link_deps, created_at FROM plans WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to get plan: {}", e)))
    }

    pub async fn list_plans(&self) -> Result<Vec<PlanRecord>> {
        query_as::<_, PlanRecord>("SELECT id, name, build_deps, link_deps, created_at FROM plans ORDER BY name")
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
            return Err(WrightError::PartNotFound(name.to_string()));
        }
        Ok(())
    }

    pub async fn get_parts_by_plan_id(&self, plan_id: i64) -> Result<Vec<super::InstalledPart>> {
        let sql = format!("SELECT {} FROM parts WHERE plan_id = ? ORDER BY name", PART_COLUMNS);
        query_as::<_, super::InstalledPart>(&sql)
            .bind(plan_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to get parts by plan_id: {}", e)))
    }
}
