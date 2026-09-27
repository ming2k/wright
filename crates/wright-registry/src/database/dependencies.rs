//! Dependency, conflict, replacement, and orphan queries.

use super::{Dependency, InstalledDb, ReadOnlyDb};
use crate::error::{Result, WrightError};
use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use rusqlite::params;
use std::collections::HashSet;

impl ReadOnlyDb {
    pub async fn check_dependency(&self, name: &str) -> Result<bool> {
        let name = name.to_string();
        self.read(move |conn| {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM parts WHERE name = ?1",
                    params![name],
                    |r| r.get(0),
                )
                .map_err(|e| WrightError::context("failed to check part dependency", e))?;
            Ok(count > 0)
        })
        .await
    }

    pub async fn get_dependents(&self, name: &str) -> Result<Vec<String>> {
        let name = name.to_string();
        self.read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT p.name FROM dependencies d
                 JOIN parts p ON d.part_id = p.id
                 WHERE d.depends_on = ?1",
            )?;
            let rows = stmt.query_map(params![name], |r| r.get(0))?;
            let mut result = Vec::new();
            for r in rows {
                result.push(r?);
            }
            Ok(result)
        })
        .await
    }

    pub async fn get_dependencies(&self, part_id: i64) -> Result<Vec<Dependency>> {
        self.read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT depends_on, version_constraint FROM dependencies WHERE part_id = ?1",
            )?;
            let rows = stmt.query_map(params![part_id], Dependency::from_row)?;
            let mut result = Vec::new();
            for r in rows {
                result.push(r?);
            }
            Ok(result)
        })
        .await
    }

    pub async fn get_dependencies_by_name(&self, name: &str) -> Result<Vec<Dependency>> {
        let name = name.to_string();
        self.read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT d.depends_on, d.version_constraint
                 FROM dependencies d
                 JOIN parts p ON d.part_id = p.id
                 WHERE p.name = ?1",
            )?;
            let rows = stmt.query_map(params![name], Dependency::from_row)?;
            let mut result = Vec::new();
            for r in rows {
                result.push(r?);
            }
            Ok(result)
        })
        .await
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
        let name = name.to_string();
        self.read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT d.depends_on FROM dependencies d
                 JOIN parts p ON d.part_id = p.id
                 WHERE p.name = ?1
                   AND EXISTS (
                       SELECT 1 FROM parts dep WHERE dep.name = d.depends_on AND dep.origin = 'dependency'
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM dependencies d2
                       JOIN parts p2 ON d2.part_id = p2.id
                       WHERE d2.depends_on = d.depends_on AND p2.name != ?2
                   )",
            )?;
            let rows = stmt.query_map(params![name, name], |r| r.get(0))?;
            let mut result = Vec::new();
            for r in rows {
                result.push(r?);
            }
            Ok(result)
        })
        .await
    }

    pub async fn get_conflicts(&self, part_id: i64) -> Result<Vec<String>> {
        self.read(move |conn| {
            let mut stmt =
                conn.prepare("SELECT name FROM conflicts WHERE part_id = ?1 ORDER BY name")?;
            let rows = stmt.query_map(params![part_id], |r| r.get(0))?;
            let mut result = Vec::new();
            for r in rows {
                result.push(r?);
            }
            Ok(result)
        })
        .await
    }

    pub async fn find_conflicting_parts(&self, name: &str) -> Result<Vec<String>> {
        let name = name.to_string();
        self.read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT p.name FROM parts p
                 JOIN conflicts c ON p.id = c.part_id
                 WHERE c.name = ?1",
            )?;
            let rows = stmt.query_map(params![name], |r| r.get(0))?;
            let mut result = Vec::new();
            for r in rows {
                result.push(r?);
            }
            Ok(result)
        })
        .await
    }

    pub async fn get_replaces(&self, part_id: i64) -> Result<Vec<String>> {
        self.read(move |conn| {
            let mut stmt =
                conn.prepare("SELECT name FROM replaces WHERE part_id = ?1 ORDER BY name")?;
            let rows = stmt.query_map(params![part_id], |r| r.get(0))?;
            let mut result = Vec::new();
            for r in rows {
                result.push(r?);
            }
            Ok(result)
        })
        .await
    }
}

impl InstalledDb {
    pub async fn insert_dependencies(&self, part_id: i64, deps: &[Dependency]) -> Result<()> {
        let deps = deps.to_vec();
        self.write(move |conn| {
            let mut stmt = conn.prepare(
                "INSERT INTO dependencies (part_id, depends_on, version_constraint) VALUES (?1, ?2, ?3)",
            )?;
            for dep in deps {
                stmt.execute(params![part_id, dep.name, dep.version_constraint])
                    .map_err(|e| WrightError::context("failed to insert dependency", e))?;
            }
            Ok(())
        })
        .await
    }

    pub async fn replace_dependencies(&self, part_id: i64, deps: &[Dependency]) -> Result<()> {
        let deps = deps.to_vec();
        self.write(move |conn| {
            let tx = conn
                .transaction()
                .map_err(|e| WrightError::context("failed to begin replace dependencies tx", e))?;

            tx.execute("DELETE FROM dependencies WHERE part_id = ?1", params![part_id])
                .map_err(|e| WrightError::context("failed to delete old dependencies", e))?;

            {
                let mut stmt = tx.prepare(
                    "INSERT INTO dependencies (part_id, depends_on, version_constraint) VALUES (?1, ?2, ?3)",
                )?;
                for dep in deps {
                    stmt.execute(params![part_id, dep.name, dep.version_constraint])
                        .map_err(|e| WrightError::context("failed to insert dependency", e))?;
                }
            }

            tx.commit()
                .map_err(|e| WrightError::context("failed to commit replaced dependencies", e))?;
            Ok(())
        })
        .await
    }

    pub async fn insert_conflicts(&self, part_id: i64, names: &[String]) -> Result<()> {
        let names = names.to_vec();
        self.write(move |conn| {
            let mut stmt = conn.prepare("INSERT INTO conflicts (part_id, name) VALUES (?1, ?2)")?;
            for name in names {
                stmt.execute(params![part_id, name])
                    .map_err(|e| WrightError::context("failed to insert conflicts", e))?;
            }
            Ok(())
        })
        .await
    }

    pub async fn replace_conflicts(&self, part_id: i64, names: &[String]) -> Result<()> {
        let names = names.to_vec();
        self.write(move |conn| {
            let tx = conn
                .transaction()
                .map_err(|e| WrightError::context("failed to begin replace conflicts tx", e))?;

            tx.execute("DELETE FROM conflicts WHERE part_id = ?1", params![part_id])
                .map_err(|e| WrightError::context("failed to delete old conflicts", e))?;

            {
                let mut stmt =
                    tx.prepare("INSERT INTO conflicts (part_id, name) VALUES (?1, ?2)")?;
                for name in names {
                    stmt.execute(params![part_id, name])
                        .map_err(|e| WrightError::context("failed to insert conflicts", e))?;
                }
            }

            tx.commit()
                .map_err(|e| WrightError::context("failed to commit replaced conflicts", e))?;
            Ok(())
        })
        .await
    }

    pub async fn insert_replaces(&self, part_id: i64, names: &[String]) -> Result<()> {
        let names = names.to_vec();
        self.write(move |conn| {
            let mut stmt = conn.prepare("INSERT INTO replaces (part_id, name) VALUES (?1, ?2)")?;
            for name in names {
                stmt.execute(params![part_id, name])
                    .map_err(|e| WrightError::context("failed to insert replaces", e))?;
            }
            Ok(())
        })
        .await
    }

    pub async fn replace_replaces(&self, part_id: i64, names: &[String]) -> Result<()> {
        let names = names.to_vec();
        self.write(move |conn| {
            let tx = conn
                .transaction()
                .map_err(|e| WrightError::context("failed to begin replace replaces tx", e))?;

            tx.execute("DELETE FROM replaces WHERE part_id = ?1", params![part_id])
                .map_err(|e| WrightError::context("failed to delete old replaces", e))?;

            {
                let mut stmt =
                    tx.prepare("INSERT INTO replaces (part_id, name) VALUES (?1, ?2)")?;
                for name in names {
                    stmt.execute(params![part_id, name])
                        .map_err(|e| WrightError::context("failed to insert replaces", e))?;
                }
            }

            tx.commit()
                .map_err(|e| WrightError::context("failed to commit replaced replaces", e))?;
            Ok(())
        })
        .await
    }
}
