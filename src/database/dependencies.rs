use super::{Dependency, InstalledDb};
use crate::error::{Result, WrightError};
use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use sqlx::{query, query_as};
use std::collections::HashSet;

impl InstalledDb {
    pub async fn insert_dependencies(&self, part_id: i64, deps: &[Dependency]) -> Result<()> {
        for dep in deps {
            query("INSERT INTO dependencies (part_id, depends_on, version_constraint) VALUES (?, ?, ?)")
                .bind(part_id)
                .bind(&dep.name)
                .bind(&dep.version_constraint)
                .execute(&self.pool)
                .await
                .map_err(|e| WrightError::DatabaseError(format!("failed to insert dependency: {}", e)))?;
        }
        Ok(())
    }

    pub async fn replace_dependencies(&self, part_id: i64, deps: &[Dependency]) -> Result<()> {
        query("DELETE FROM dependencies WHERE part_id = ?")
            .bind(part_id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to delete old dependencies: {}", e))
            })?;

        self.insert_dependencies(part_id, deps).await
    }

    pub async fn check_dependency(&self, name: &str) -> Result<bool> {
        let row = query("SELECT COUNT(*) as count FROM parts WHERE name = ?")
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to check part dependency: {}", e))
            })?;

        use sqlx::Row;
        let count: i64 = row
            .try_get(0)
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?;

        Ok(count > 0)
    }

    pub async fn get_dependents(&self, name: &str) -> Result<Vec<String>> {
        let rows = query(
            "SELECT DISTINCT p.name FROM dependencies d
             JOIN parts p ON d.part_id = p.id
             WHERE d.depends_on = ?",
        )
        .bind(name)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to get dependents: {}", e)))?;

        let mut result = Vec::new();
        for row in rows {
            use sqlx::Row;
            let name: String = row
                .try_get(0)
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            result.push(name);
        }
        Ok(result)
    }

    pub async fn get_dependencies(&self, part_id: i64) -> Result<Vec<Dependency>> {
        query_as::<_, Dependency>(
            "SELECT depends_on as \"depends_on\", version_constraint FROM dependencies WHERE part_id = ?")
            .bind(part_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to get dependencies: {}", e)))
    }

    pub async fn get_dependencies_by_name(&self, name: &str) -> Result<Vec<Dependency>> {
        query_as::<_, Dependency>(
            "SELECT d.depends_on as \"depends_on\", d.version_constraint
             FROM dependencies d
             JOIN parts p ON d.part_id = p.id
             WHERE p.name = ?",
        )
        .bind(name)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to get dependencies: {}", e)))
    }

    pub async fn get_recursive_dependents(&self, name: &str) -> Result<Vec<String>> {
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        visited.insert(name.to_string());
        self.collect_dependents_recursive(name, &mut visited, &mut result)
            .await?;
        Ok(result)
    }

    fn collect_dependents_recursive<'a>(
        &'a self,
        name: &'a str,
        visited: &'a mut HashSet<String>,
        result: &'a mut Vec<String>,
    ) -> BoxFuture<'a, Result<()>> {
        async move {
            let dependents = self.get_dependents(name).await?;
            for dep_name in &dependents {
                if visited.contains(dep_name) {
                    continue;
                }
                visited.insert(dep_name.to_string());
                self.collect_dependents_recursive(dep_name, visited, result)
                    .await?;
                result.push(dep_name.to_string());
            }
            Ok(())
        }
        .boxed()
    }

    pub async fn get_orphan_dependencies(&self, name: &str) -> Result<Vec<String>> {
        let rows = query(
            "SELECT d.depends_on FROM dependencies d
             JOIN parts p ON d.part_id = p.id
             WHERE p.name = ?
               AND EXISTS (
                   SELECT 1 FROM parts dep WHERE dep.name = d.depends_on AND dep.origin = 'dependency'
               )
               AND NOT EXISTS (
                   SELECT 1 FROM dependencies d2
                   JOIN parts p2 ON d2.part_id = p2.id
                   WHERE d2.depends_on = d.depends_on AND p2.name != ?
               )")
            .bind(name)
            .bind(name)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to get orphan deps: {}", e)))?;

        let mut result = Vec::new();
        for row in rows {
            use sqlx::Row;
            result.push(
                row.try_get(0)
                    .map_err(|e| WrightError::DatabaseError(e.to_string()))?,
            );
        }
        Ok(result)
    }

    pub async fn insert_conflicts(&self, part_id: i64, names: &[String]) -> Result<()> {
        for name in names {
            query("INSERT INTO conflicts (part_id, name) VALUES (?, ?)")
                .bind(part_id)
                .bind(name)
                .execute(&self.pool)
                .await
                .map_err(|e| {
                    WrightError::DatabaseError(format!("failed to insert conflicts: {}", e))
                })?;
        }
        Ok(())
    }

    pub async fn get_conflicts(&self, part_id: i64) -> Result<Vec<String>> {
        let rows = query("SELECT name FROM conflicts WHERE part_id = ? ORDER BY name")
            .bind(part_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to get conflicts: {}", e)))?;

        let mut result = Vec::new();
        for row in rows {
            use sqlx::Row;
            result.push(
                row.try_get(0)
                    .map_err(|e| WrightError::DatabaseError(e.to_string()))?,
            );
        }
        Ok(result)
    }

    pub async fn find_conflicting_parts(&self, name: &str) -> Result<Vec<String>> {
        let rows = query(
            "SELECT p.name FROM parts p
             JOIN conflicts c ON p.id = c.part_id
             WHERE c.name = ?",
        )
        .bind(name)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            WrightError::DatabaseError(format!("failed to find conflicting parts: {}", e))
        })?;

        let mut result = Vec::new();
        for row in rows {
            use sqlx::Row;
            result.push(
                row.try_get(0)
                    .map_err(|e| WrightError::DatabaseError(e.to_string()))?,
            );
        }
        Ok(result)
    }

    pub async fn insert_replaces(&self, part_id: i64, names: &[String]) -> Result<()> {
        for name in names {
            query("INSERT INTO replaces (part_id, name) VALUES (?, ?)")
                .bind(part_id)
                .bind(name)
                .execute(&self.pool)
                .await
                .map_err(|e| {
                    WrightError::DatabaseError(format!("failed to insert replaces: {}", e))
                })?;
        }
        Ok(())
    }

    pub async fn replace_replaces(&self, part_id: i64, names: &[String]) -> Result<()> {
        query("DELETE FROM replaces WHERE part_id = ?")
            .bind(part_id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to delete old replaces: {}", e))
            })?;

        self.insert_replaces(part_id, names).await
    }

    pub async fn get_replaces(&self, part_id: i64) -> Result<Vec<String>> {
        let rows = query("SELECT name FROM replaces WHERE part_id = ? ORDER BY name")
            .bind(part_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to get replaces: {}", e)))?;

        let mut result = Vec::new();
        for row in rows {
            use sqlx::Row;
            result.push(
                row.try_get(0)
                    .map_err(|e| WrightError::DatabaseError(e.to_string()))?,
            );
        }
        Ok(result)
    }
}
