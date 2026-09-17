use super::{InstalledDb, NewPlan, NewPlanProvenance, RegisterPlan};
use crate::error::{Result, WrightError};
use rusqlite::params;

#[derive(Debug, Clone)]
pub struct PlanRecord {
    pub id: i64,
    pub name: String,
    pub version: String,
    pub release: i64,
    pub epoch: i64,
    pub arch: String,
    pub registered_at: Option<String>,
    /// SHA-256 of the plan source that produced the registered parts, from
    /// `.PARTINFO` `[provenance]`. NULL for parts sealed before ADR-0023.
    pub plan_checksum: Option<String>,
}

impl PlanRecord {
    pub fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            version: row.get(2)?,
            release: row.get(3)?,
            epoch: row.get(4)?,
            arch: row.get(5)?,
            registered_at: row.get(6)?,
            plan_checksum: row.get(7)?,
        })
    }
}

impl InstalledDb {
    pub async fn insert_plan(&self, plan: NewPlan<'_>) -> Result<i64> {
        let name = plan.name.to_string();
        let version = plan.version.to_string();
        let release = plan.release as i64;
        let epoch = plan.epoch as i64;
        let arch = plan.arch.to_string();

        self.write(move |conn| {
            let res = conn.execute(
                "INSERT INTO plans (name, version, release, epoch, arch)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![name, version, release, epoch, arch],
            );
            match res {
                Ok(_) => Ok(conn.last_insert_rowid()),
                Err(e) => {
                    if let rusqlite::Error::SqliteFailure(ref err, _) = e
                        && err.code == rusqlite::ErrorCode::ConstraintViolation
                    {
                        return Err(WrightError::DatabaseError(format!(
                            "plan '{}' already registered",
                            name
                        )));
                    }
                    Err(WrightError::context("failed to insert plan", e))
                }
            }
        })
        .await
    }

    pub async fn get_plan(&self, name: &str) -> Result<Option<PlanRecord>> {
        let name = name.to_string();
        self.read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, version, release, epoch, arch, registered_at, plan_checksum
                 FROM plans WHERE name = ?1",
            )?;
            let mut rows = stmt.query(params![name])?;
            if let Some(row) = rows.next()? {
                Ok(Some(PlanRecord::from_row(row)?))
            } else {
                Ok(None)
            }
        })
        .await
    }

    pub async fn get_plan_by_id(&self, id: i64) -> Result<Option<PlanRecord>> {
        self.read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, version, release, epoch, arch, registered_at, plan_checksum
                 FROM plans WHERE id = ?1",
            )?;
            let mut rows = stmt.query(params![id])?;
            if let Some(row) = rows.next()? {
                Ok(Some(PlanRecord::from_row(row)?))
            } else {
                Ok(None)
            }
        })
        .await
    }

    pub async fn list_plans(&self) -> Result<Vec<PlanRecord>> {
        self.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, version, release, epoch, arch, registered_at, plan_checksum
                 FROM plans ORDER BY name",
            )?;
            let rows = stmt.query_map([], PlanRecord::from_row)?;
            let mut plans = Vec::new();
            for r in rows {
                plans.push(r?);
            }
            Ok(plans)
        })
        .await
    }

    pub async fn remove_plan(&self, name: &str) -> Result<()> {
        let name_owned = name.to_string();
        self.write(move |conn| {
            let rows_affected = conn.execute("DELETE FROM plans WHERE name = ?1", params![name_owned])
                .map_err(|e| WrightError::context("failed to remove plan", e))?;
            if rows_affected == 0 {
                return Err(WrightError::DatabaseError(format!(
                    "plan not found: {}",
                    name_owned
                )));
            }
            Ok(())
        })
        .await
    }

    pub async fn remove_plan_by_id(&self, id: i64) -> Result<()> {
        self.write(move |conn| {
            let rows_affected = conn.execute("DELETE FROM plans WHERE id = ?1", params![id])
                .map_err(|e| WrightError::context("failed to remove plan by id", e))?;
            if rows_affected == 0 {
                return Err(WrightError::DatabaseError(format!(
                    "plan not found: id {}",
                    id
                )));
            }
            Ok(())
        })
        .await
    }

    pub async fn get_parts_by_plan_id(&self, plan_id: i64) -> Result<Vec<super::InstalledPart>> {
        use super::PART_COLUMNS;
        let sql = format!(
            "SELECT {} FROM parts WHERE plan_id = ?1 ORDER BY name",
            PART_COLUMNS
        );
        self.read(move |conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![plan_id], super::InstalledPart::from_row)?;
            let mut parts = Vec::new();
            for r in rows {
                parts.push(r?);
            }
            Ok(parts)
        })
        .await
    }

    pub async fn get_plan_id_by_name(&self, name: &str) -> Result<Option<i64>> {
        let name = name.to_string();
        self.read(move |conn| {
            let mut stmt = conn.prepare("SELECT id FROM plans WHERE name = ?1")?;
            let mut rows = stmt.query(params![name])?;
            if let Some(row) = rows.next()? {
                let id: i64 = row.get(0)?;
                Ok(Some(id))
            } else {
                Ok(None)
            }
        })
        .await
    }

    /// Ensure a plan is registered in the database from part metadata.
    /// If the plan already exists, updates its version metadata to match the part.
    pub async fn ensure_plan_registered(&self, registration: RegisterPlan<'_>) -> Result<i64> {
        let plan = registration.plan;
        let plan_id = if let Some(existing) = self.get_plan(plan.name).await? {
            let version = plan.version.to_string();
            let release = plan.release as i64;
            let epoch = plan.epoch as i64;
            let arch = plan.arch.to_string();
            let id = existing.id;
            self.write(move |conn| {
                conn.execute(
                    "UPDATE plans SET version = ?1, release = ?2, epoch = ?3, arch = ?4 WHERE id = ?5",
                    params![version, release, epoch, arch, id],
                )
                .map_err(|e| WrightError::context("failed to update plan", e))?;
                Ok(id)
            })
            .await?
        } else {
            self.insert_plan(plan).await?
        };

        if let Some(provenance) = registration.provenance {
            self.set_plan_provenance(plan_id, provenance).await?;
        }
        Ok(plan_id)
    }

    /// Mirror the `[provenance]` section of `.PARTINFO` onto the plan row
    /// (ADR-0023). Descriptive audit data; parts sealed before ADR-0023 have
    /// no provenance and leave the columns NULL.
    pub async fn set_plan_provenance(
        &self,
        plan_id: i64,
        provenance: NewPlanProvenance<'_>,
    ) -> Result<()> {
        let source_checksums = serde_json::to_string(&provenance.source_checksums)
            .map_err(|e| WrightError::context("serialize source_checksums", e))?;
        let plan_checksum = provenance.plan_checksum.map(|s| s.to_string());
        let wright_version = provenance.wright_version.to_string();
        let isolation = provenance.isolation.to_string();

        self.write(move |conn| {
            conn.execute(
                "UPDATE plans SET plan_checksum = ?1, source_checksums = ?2,
                        wright_version = ?3, isolation = ?4 WHERE id = ?5",
                params![plan_checksum, source_checksums, wright_version, isolation, plan_id],
            )
            .map_err(|e| WrightError::context("failed to set plan provenance", e))?;
            Ok(())
        })
        .await
    }
}
