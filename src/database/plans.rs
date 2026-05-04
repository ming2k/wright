use super::{InstalledDb, PART_COLUMNS};
use crate::error::{Result, WrightError};
use crate::part::part::PartInfo;
use sqlx::{query, query_as};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PlanRecord {
    pub id: i64,
    pub name: String,
    pub version: String,
    pub release: i64,
    pub epoch: i64,
    pub description: Option<String>,
    pub arch: String,
    pub license: Option<String>,
    pub url: Option<String>,
    pub build_deps: Option<String>,
    pub link_deps: Option<String>,
    pub registered_at: Option<String>,
}

impl InstalledDb {
    pub async fn insert_plan(
        &self,
        name: &str,
        version: &str,
        release: u32,
        epoch: u32,
        description: &str,
        arch: &str,
        license: &str,
        url: Option<&str>,
        build_deps: Option<&str>,
        link_deps: Option<&str>,
    ) -> Result<i64> {
        let res = query(
            "INSERT INTO plans (name, version, release, epoch, description, arch, license, url, build_deps, link_deps)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(name)
            .bind(version)
            .bind(release as i64)
            .bind(epoch as i64)
            .bind(description)
            .bind(arch)
            .bind(license)
            .bind(url)
            .bind(build_deps)
            .bind(link_deps)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref db_err) = e {
                if db_err.is_unique_violation() {
                    return WrightError::DatabaseError(format!("plan '{}' already registered", name));
                }
            }
            WrightError::DatabaseError(format!("failed to insert plan: {}", e))
        })?;

        Ok(res.last_insert_rowid())
    }

    pub async fn get_plan(&self, name: &str
    ) -> Result<Option<PlanRecord>> {
        query_as::<_, PlanRecord>(
            "SELECT id, name, version, release, epoch, description, arch, license, url, build_deps, link_deps, registered_at
             FROM plans WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to get plan: {}", e)))
    }

    pub async fn get_plan_by_id(&self, id: i64
    ) -> Result<Option<PlanRecord>> {
        query_as::<_, PlanRecord>(
            "SELECT id, name, version, release, epoch, description, arch, license, url, build_deps, link_deps, registered_at
             FROM plans WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to get plan by id: {}", e)))
    }

    pub async fn list_plans(&self) -> Result<Vec<PlanRecord>> {
        query_as::<_, PlanRecord>(
            "SELECT id, name, version, release, epoch, description, arch, license, url, build_deps, link_deps, registered_at
             FROM plans ORDER BY name")
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

    pub async fn get_plan_id_by_name(&self, name: &str) -> Result<Option<i64>> {
        let row = query("SELECT id FROM plans WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to query plan id: {}", e)))?;

        match row {
            Some(r) => {
                use sqlx::Row;
                let id: i64 = r.try_get(0).map_err(|e| WrightError::DatabaseError(e.to_string()))?;
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
    ) -> Result<i64> {
        if let Some(existing) = self.get_plan(&partinfo.plan_name).await? {
            let build_deps_json = if partinfo.build_deps.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&partinfo.build_deps).map_err(|e| {
                    WrightError::DatabaseError(format!("failed to serialize build_deps: {}", e))
                })?)
            };
            let link_deps_json = if partinfo.link_deps.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&partinfo.link_deps).map_err(|e| {
                    WrightError::DatabaseError(format!("failed to serialize link_deps: {}", e))
                })?)
            };
            query(
                "UPDATE plans SET version = ?, release = ?, epoch = ?, description = ?, arch = ?, license = ?, build_deps = ?, link_deps = ? WHERE id = ?"
            )
            .bind(&partinfo.plan_version)
            .bind(partinfo.plan_release as i64)
            .bind(partinfo.plan_epoch as i64)
            .bind(&partinfo.description)
            .bind(&partinfo.arch)
            .bind(&partinfo.license)
            .bind(build_deps_json.as_deref())
            .bind(link_deps_json.as_deref())
            .bind(existing.id)
            .execute(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to update plan: {}", e)))?;
            Ok(existing.id)
        } else {
            let build_deps_json = if partinfo.build_deps.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&partinfo.build_deps).map_err(|e| {
                    WrightError::DatabaseError(format!("failed to serialize build_deps: {}", e))
                })?)
            };
            let link_deps_json = if partinfo.link_deps.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&partinfo.link_deps).map_err(|e| {
                    WrightError::DatabaseError(format!("failed to serialize link_deps: {}", e))
                })?)
            };
            self.insert_plan(
                &partinfo.plan_name,
                &partinfo.plan_version,
                partinfo.plan_release,
                partinfo.plan_epoch,
                &partinfo.description,
                &partinfo.arch,
                &partinfo.license,
                None,
                build_deps_json.as_deref(),
                link_deps_json.as_deref(),
            )
            .await
        }
    }
}
