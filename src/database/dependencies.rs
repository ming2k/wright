use std::collections::HashSet;

use crate::error::{Result, WrightError};

use super::{DepType, Dependency, InstalledDb};

impl InstalledDb {
    pub fn insert_dependencies(&self, part_id: i64, deps: &[Dependency]) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "INSERT INTO dependencies (part_id, depends_on, version_constraint, dep_type)
             VALUES (?1, ?2, ?3, ?4)",
        )?;

        for dep in deps {
            stmt.execute(rusqlite::params![
                part_id,
                dep.name,
                dep.constraint,
                dep.dep_type
            ])?;
        }

        Ok(())
    }

    pub fn replace_dependencies(&self, part_id: i64, deps: &[Dependency]) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM dependencies WHERE part_id = ?1",
                rusqlite::params![part_id],
            )
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to delete old dependencies: {}", e))
            })?;
        self.insert_dependencies(part_id, deps)
    }

    pub fn check_dependency(&self, name: &str) -> Result<bool> {
        let mut stmt = self
            .conn
            .prepare("SELECT COUNT(*) FROM parts WHERE name = ?1")?;
        let count: i64 = stmt.query_row(rusqlite::params![name], |row| row.get(0))?;
        if count > 0 {
            return Ok(true);
        }
        let mut stmt2 = self
            .conn
            .prepare("SELECT COUNT(*) FROM provides WHERE name = ?1")?;
        let prov_count: i64 = stmt2.query_row(rusqlite::params![name], |row| row.get(0))?;
        Ok(prov_count > 0)
    }

    pub fn get_dependents(&self, name: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT p.name, d.dep_type FROM dependencies d
             JOIN parts p ON d.part_id = p.id
             WHERE d.depends_on = ?1",
        )?;

        let rows = stmt
            .query_map(rusqlite::params![name], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to get dependents: {}", e)))?;

        Ok(rows)
    }

    pub fn get_dependencies(&self, part_id: i64) -> Result<Vec<Dependency>> {
        let mut stmt = self.conn.prepare(
            "SELECT depends_on, version_constraint, dep_type FROM dependencies WHERE part_id = ?1",
        )?;

        let rows = stmt
            .query_map(rusqlite::params![part_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to get dependencies: {}", e))
            })?;

        Ok(rows
            .into_iter()
            .map(|(name, constraint, dt_str)| Dependency {
                name,
                constraint,
                dep_type: DepType::try_from(dt_str.as_str()).unwrap_or(DepType::Runtime),
            })
            .collect())
    }

    pub fn get_dependencies_by_name(&self, name: &str) -> Result<Vec<Dependency>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.depends_on, d.version_constraint, d.dep_type
             FROM dependencies d
             JOIN parts p ON d.part_id = p.id
             WHERE p.name = ?1",
        )?;

        let rows = stmt
            .query_map(rusqlite::params![name], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to get dependencies: {}", e))
            })?;

        Ok(rows
            .into_iter()
            .map(|(n, constraint, dt_str)| Dependency {
                name: n,
                constraint,
                dep_type: DepType::try_from(dt_str.as_str()).unwrap_or(DepType::Runtime),
            })
            .collect())
    }

    pub fn get_recursive_dependents(&self, name: &str) -> Result<Vec<String>> {
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        visited.insert(name.to_string());
        self.collect_dependents_recursive(name, &mut visited, &mut result)?;
        Ok(result)
    }

    fn collect_dependents_recursive(
        &self,
        name: &str,
        visited: &mut HashSet<String>,
        result: &mut Vec<String>,
    ) -> Result<()> {
        let dependents = self.get_dependents(name)?;
        for (dep_name, _) in &dependents {
            if visited.contains(dep_name) {
                continue;
            }
            visited.insert(dep_name.to_string());
            self.collect_dependents_recursive(dep_name, visited, result)?;
            result.push(dep_name.to_string());
        }
        Ok(())
    }

    pub fn get_orphan_dependencies(&self, name: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.depends_on FROM dependencies d
             JOIN parts p ON d.part_id = p.id
             WHERE p.name = ?1
               AND EXISTS (
                   SELECT 1 FROM parts dep WHERE dep.name = d.depends_on AND dep.origin = 'dependency'
               )
               AND NOT EXISTS (
                   SELECT 1 FROM dependencies d2
                   JOIN parts p2 ON d2.part_id = p2.id
                   WHERE d2.depends_on = d.depends_on AND p2.name != ?1
               )"
        ).map_err(|e| WrightError::DatabaseError(format!("failed to prepare orphan deps query: {}", e)))?;

        let rows = stmt
            .query_map(rusqlite::params![name], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to get orphan deps: {}", e)))?;

        Ok(rows)
    }

    pub fn insert_optional_dependencies(&self, part_id: i64, deps: &[String]) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare("INSERT INTO optional_dependencies (part_id, name) VALUES (?1, ?2)")?;
        for name in deps {
            stmt.execute(rusqlite::params![part_id, name])?;
        }
        Ok(())
    }

    pub fn replace_optional_dependencies(&self, part_id: i64, deps: &[String]) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM optional_dependencies WHERE part_id = ?1",
                rusqlite::params![part_id],
            )
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to delete old optional deps: {}", e))
            })?;
        self.insert_optional_dependencies(part_id, deps)
    }

    pub fn get_optional_dependencies(&self, part_id: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM optional_dependencies WHERE part_id = ?1 ORDER BY name")?;
        let rows = stmt
            .query_map(rusqlite::params![part_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to get optional deps: {}", e))
            })?;
        Ok(rows)
    }

    pub fn insert_provides(&self, part_id: i64, names: &[String]) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare("INSERT INTO provides (part_id, name) VALUES (?1, ?2)")?;
        for name in names {
            stmt.execute(rusqlite::params![part_id, name])?;
        }
        Ok(())
    }

    pub fn get_provides(&self, part_id: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM provides WHERE part_id = ?1 ORDER BY name")?;
        let rows = stmt
            .query_map(rusqlite::params![part_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to get provides: {}", e)))?;
        Ok(rows)
    }

    pub fn find_providers(&self, virtual_name: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.name FROM parts p
             JOIN provides pv ON p.id = pv.part_id
             WHERE pv.name = ?1",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![virtual_name], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to find providers: {}", e)))?;
        Ok(rows)
    }

    pub fn insert_conflicts(&self, part_id: i64, names: &[String]) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare("INSERT INTO conflicts (part_id, name) VALUES (?1, ?2)")?;
        for name in names {
            stmt.execute(rusqlite::params![part_id, name])?;
        }
        Ok(())
    }

    pub fn get_conflicts(&self, part_id: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM conflicts WHERE part_id = ?1 ORDER BY name")?;
        let rows = stmt
            .query_map(rusqlite::params![part_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to get conflicts: {}", e)))?;
        Ok(rows)
    }

    pub fn find_conflicting_parts(&self, name: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.name FROM parts p
             JOIN conflicts c ON p.id = c.part_id
             WHERE c.name = ?1",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![name], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to find conflicting parts: {}", e))
            })?;
        Ok(rows)
    }

    pub fn insert_replaces(&self, part_id: i64, names: &[String]) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare("INSERT INTO replaces (part_id, name) VALUES (?1, ?2)")?;
        for name in names {
            stmt.execute(rusqlite::params![part_id, name])?;
        }
        Ok(())
    }

    pub fn replace_replaces(&self, part_id: i64, names: &[String]) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM replaces WHERE part_id = ?1",
                rusqlite::params![part_id],
            )
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to delete old replaces: {}", e))
            })?;
        self.insert_replaces(part_id, names)
    }

    pub fn get_replaces(&self, part_id: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM replaces WHERE part_id = ?1 ORDER BY name")?;
        let rows = stmt
            .query_map(rusqlite::params![part_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| WrightError::DatabaseError(format!("failed to get replaces: {}", e)))?;
        Ok(rows)
    }
}
