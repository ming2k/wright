//! Installed-part persistence and lookup.

use super::{InstalledDb, InstalledPart, NewPart, Origin, PART_COLUMNS, PartWithPlan};
use crate::error::{Result, WrightError};
use rusqlite::params;

const PART_WITH_PLAN_SQL: &str = "
    SELECT
        p.id, p.name, p.plan_id, p.installed_at, p.part_hash, p.deploy_scripts, p.origin,
        pl.name as plan_name, pl.version, pl.release, pl.epoch, pl.arch
    FROM parts p
    INNER JOIN plans pl ON p.plan_id = pl.id
";

impl InstalledDb {
    pub async fn insert_part(&self, part: NewPart<'_>) -> Result<i64> {
        let name = part.name.to_string();
        let plan_id = part.plan_id;
        let part_hash = part.part_hash.map(|s| s.to_string());
        let deploy_scripts = part.deploy_scripts.map(|s| s.to_string());
        let origin = part.origin;

        self.write(move |conn| {
            // Remove any external placeholder with this name before inserting the real record.
            conn.execute(
                "DELETE FROM parts WHERE name = ?1 AND origin = 'external'",
                params![name],
            )
            .map_err(|e| WrightError::context("failed to clear external placeholder", e))?;

            let res = conn.execute(
                "INSERT INTO parts (name, plan_id, part_hash, deploy_scripts, origin)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![name, plan_id, part_hash, deploy_scripts, origin],
            );

            match res {
                Ok(_) => Ok(conn.last_insert_rowid()),
                Err(e) => {
                    if let rusqlite::Error::SqliteFailure(ref err, _) = e
                        && err.code == rusqlite::ErrorCode::ConstraintViolation
                    {
                        return Err(WrightError::PartAlreadyInstalled(name));
                    }
                    Err(WrightError::context("failed to insert part", e))
                }
            }
        })
        .await
    }

    pub async fn provide_part(&self, name: &str, version: &str) -> Result<()> {
        let name = name.to_string();
        let version = version.to_string();

        self.write(move |conn| {
            // Refuse to overwrite a genuinely installed part.
            let mut check_stmt = conn.prepare("SELECT origin FROM parts WHERE name = ?1")?;
            let mut rows = check_stmt.query(params![name])?;
            if let Some(row) = rows.next()? {
                let origin: Origin = row.get(0)?;
                if origin != Origin::External {
                    return Err(WrightError::PartAlreadyInstalled(format!(
                        "{} is already installed; uninstall it before providing",
                        name
                    )));
                }
            }
            drop(rows);
            drop(check_stmt);

            let plan_id = {
                let mut plan_stmt = conn.prepare("SELECT id FROM plans WHERE name = ?1")?;
                let mut plan_rows = plan_stmt.query(params![name])?;
                let existing_id: Option<i64> = if let Some(row) = plan_rows.next()? {
                    Some(row.get(0)?)
                } else {
                    None
                };
                drop(plan_rows);
                drop(plan_stmt);

                match existing_id {
                    Some(id) => {
                        let deployed: i64 = conn.query_row(
                            "SELECT COUNT(*) FROM parts WHERE plan_id = ?1 AND origin != 'external'",
                            params![id],
                            |r| r.get(0),
                        )
                        .map_err(|e| WrightError::context("failed to count plan parts", e))?;

                        if deployed > 0 {
                            return Err(WrightError::PartAlreadyInstalled(format!(
                                "plan '{}' has deployed parts; remove it first (`wright remove {}`) before providing it externally",
                                name, name
                            )));
                        }

                        conn.execute("UPDATE plans SET version = ?1 WHERE id = ?2", params![version, id])
                            .map_err(|e| WrightError::context("failed to update plan", e))?;
                        id
                    }
                    None => {
                        conn.execute(
                            "INSERT INTO plans (name, version, release, epoch, arch)
                             VALUES (?1, ?2, 0, 0, 'any')",
                            params![name, version],
                        )
                        .map_err(|e| WrightError::context("failed to insert plan", e))?;
                        conn.last_insert_rowid()
                    }
                }
            };

            conn.execute(
                "INSERT INTO parts (name, plan_id, part_hash, deploy_scripts, origin)
                 VALUES (?1, ?2, NULL, NULL, 'external')
                 ON CONFLICT(name) DO UPDATE SET origin = 'external', plan_id = excluded.plan_id",
                params![name, plan_id],
            )
            .map_err(|e| WrightError::context("failed to register external part", e))?;

            Ok(())
        })
        .await
    }

    pub async fn update_part(&self, part: NewPart<'_>) -> Result<()> {
        let name = part.name.to_string();
        let plan_id = part.plan_id;
        let part_hash = part.part_hash.map(|s| s.to_string());
        let deploy_scripts = part.deploy_scripts.map(|s| s.to_string());
        let origin = part.origin;

        self.write(move |conn| {
            let rows_affected = conn.execute(
                "UPDATE parts SET plan_id = ?1, part_hash = ?2, deploy_scripts = ?3, origin = ?4
                 WHERE name = ?5",
                params![plan_id, part_hash, deploy_scripts, origin, name],
            )
            .map_err(|e| WrightError::context("failed to update part", e))?;

            if rows_affected == 0 {
                return Err(WrightError::PartNotFound(name));
            }
            Ok(())
        })
        .await
    }

    pub async fn remove_part(&self, name: &str) -> Result<()> {
        let name = name.to_string();
        self.write(move |conn| {
            // Find part_id and plan_id first
            let plan_info: Option<i64> = {
                let mut stmt = conn.prepare("SELECT plan_id FROM parts WHERE name = ?1")?;
                let mut rows = stmt.query(params![name])?;
                if let Some(r) = rows.next()? {
                    Some(r.get(0)?)
                } else {
                    None
                }
            };

            let rows_affected = conn.execute("DELETE FROM parts WHERE name = ?1", params![name])
                .map_err(|e| WrightError::context("failed to remove part", e))?;

            if rows_affected == 0 {
                return Err(WrightError::PartNotFound(name));
            }

            if let Some(plan_id) = plan_info {
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM parts WHERE plan_id = ?1",
                    params![plan_id],
                    |r| r.get(0),
                )?;
                if count == 0 {
                    let _ = conn.execute("DELETE FROM plans WHERE id = ?1", params![plan_id]);
                }
            }

            Ok(())
        })
        .await
    }

    pub async fn get_part(&self, name: &str) -> Result<Option<InstalledPart>> {
        let sql = format!("SELECT {} FROM parts WHERE name = ?1", PART_COLUMNS);
        let name = name.to_string();
        self.read(move |conn| {
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(params![name])?;
            if let Some(row) = rows.next()? {
                Ok(Some(InstalledPart::from_row(row)?))
            } else {
                Ok(None)
            }
        })
        .await
    }

    pub async fn get_part_with_plan(&self, name: &str) -> Result<Option<PartWithPlan>> {
        let sql = format!("{} WHERE p.name = ?1", PART_WITH_PLAN_SQL);
        let name = name.to_string();
        self.read(move |conn| {
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(params![name])?;
            if let Some(row) = rows.next()? {
                Ok(Some(PartWithPlan::from_row(row)?))
            } else {
                Ok(None)
            }
        })
        .await
    }

    pub async fn list_parts(&self) -> Result<Vec<PartWithPlan>> {
        let sql = format!("{} ORDER BY p.name", PART_WITH_PLAN_SQL);
        self.read(move |conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], PartWithPlan::from_row)?;
            let mut parts = Vec::new();
            for r in rows {
                parts.push(r?);
            }
            Ok(parts)
        })
        .await
    }

    pub async fn get_root_parts(&self) -> Result<Vec<PartWithPlan>> {
        let sql = format!(
            "{} WHERE p.name NOT IN (SELECT DISTINCT depends_on FROM dependencies) ORDER BY p.name",
            PART_WITH_PLAN_SQL
        );
        self.read(move |conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], PartWithPlan::from_row)?;
            let mut parts = Vec::new();
            for r in rows {
                parts.push(r?);
            }
            Ok(parts)
        })
        .await
    }

    pub async fn set_origin(&self, name: &str, new_origin: Origin) -> Result<()> {
        let name = name.to_string();
        self.write(move |conn| {
            // Check existing
            let mut check = conn.prepare("SELECT origin FROM parts WHERE name = ?1")?;
            let mut rows = check.query(params![name])?;
            if let Some(row) = rows.next()? {
                let existing: Origin = row.get(0)?;
                if existing == Origin::External || new_origin <= existing {
                    return Ok(());
                }
            }
            drop(rows);
            drop(check);

            conn.execute(
                "UPDATE parts SET origin = ?1 WHERE name = ?2",
                params![new_origin, name],
            )
            .map_err(|e| WrightError::context("failed to set origin", e))?;
            Ok(())
        })
        .await
    }

    pub async fn get_orphan_parts(&self) -> Result<Vec<PartWithPlan>> {
        let sql = format!(
            "{} WHERE p.origin = 'dependency' AND p.name NOT IN (
                SELECT depends_on FROM dependencies
            )",
            PART_WITH_PLAN_SQL
        );
        self.read(move |conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], PartWithPlan::from_row)?;
            let mut parts = Vec::new();
            for r in rows {
                parts.push(r?);
            }
            Ok(parts)
        })
        .await
    }

    pub async fn get_provided_parts(&self) -> Result<Vec<PartWithPlan>> {
        let sql = format!(
            "{} WHERE p.origin = 'external' ORDER BY p.name",
            PART_WITH_PLAN_SQL
        );
        self.read(move |conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], PartWithPlan::from_row)?;
            let mut parts = Vec::new();
            for r in rows {
                parts.push(r?);
            }
            Ok(parts)
        })
        .await
    }

    pub async fn get_parts_by_plan(&self, plan_name: &str) -> Result<Vec<PartWithPlan>> {
        let sql = format!("{} WHERE pl.name = ?1 ORDER BY p.name", PART_WITH_PLAN_SQL);
        let plan_name = plan_name.to_string();
        self.read(move |conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![plan_name], PartWithPlan::from_row)?;
            let mut parts = Vec::new();
            for r in rows {
                parts.push(r?);
            }
            Ok(parts)
        })
        .await
    }

    pub async fn remove_parts_by_plan(&self, plan_name: &str) -> Result<u64> {
        let plan_name = plan_name.to_string();
        self.write(move |conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM parts INNER JOIN plans ON parts.plan_id = plans.id WHERE plans.name = ?1",
                params![plan_name],
                |r| r.get(0),
            )
            .map_err(|e| WrightError::context("failed to count parts by plan", e))?;

            conn.execute("DELETE FROM plans WHERE name = ?1", params![plan_name])
                .map_err(|e| WrightError::context("failed to remove parts by plan", e))?;

            Ok(count as u64)
        })
        .await
    }
}
