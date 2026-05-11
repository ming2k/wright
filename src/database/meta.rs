use super::{HistoryAction, HistoryRecord, HistoryStatus, InstalledDb};
use crate::error::{Result, WrightError};
use sqlx::{query, query_as};
use std::path::Path;

impl InstalledDb {
    pub fn db_path(&self) -> Option<&Path> {
        self.db_path.as_deref()
    }

    pub async fn integrity_check(&self) -> Result<Vec<String>> {
        let rows = query("PRAGMA integrity_check")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed integrity check: {}", e)))?;

        let mut results = Vec::new();
        for row in rows {
            use sqlx::Row;
            let val: String = row
                .try_get(0)
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            results.push(val);
        }

        if results.len() == 1 && results[0] == "ok" {
            Ok(Vec::new())
        } else {
            Ok(results)
        }
    }

    pub async fn get_shadowed_conflicts(&self) -> Result<Vec<String>> {
        let rows = query(
            "SELECT s.path, p1.name as original, p2.name as shadower 
             FROM shadowed_files s
             JOIN parts p1 ON s.original_owner_id = p1.id
             JOIN parts p2 ON s.shadowed_by_id = p2.id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            WrightError::DatabaseError(format!("failed to get shadowed conflicts: {}", e))
        })?;

        let mut result = Vec::new();
        for row in rows {
            use sqlx::Row;
            let path: String = row
                .try_get(0)
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            let original: String = row
                .try_get(1)
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            let shadower: String = row
                .try_get(2)
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            result.push(format!(
                "Path '{}' (owned by {}) is shadowed by {}",
                path, original, shadower
            ));
        }
        Ok(result)
    }

    pub async fn record_history(
        &self,
        session_id: &str,
        command: &str,
        part_name: &str,
        action: HistoryAction,
        old_version: Option<&str>,
        new_version: Option<&str>,
        old_hash: Option<&str>,
        new_hash: Option<&str>,
        status: HistoryStatus,
        details: Option<&str>,
    ) -> Result<i64> {
        let res = query(
            "INSERT INTO history (session_id, command, part_name, action, old_version, new_version, old_hash, new_hash, status, details)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(session_id)
            .bind(command)
            .bind(part_name)
            .bind(action)
            .bind(old_version)
            .bind(new_version)
            .bind(old_hash)
            .bind(new_hash)
            .bind(status)
            .bind(details)
        .execute(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to record history: {}", e)))?;

        Ok(res.last_insert_rowid())
    }

    pub async fn get_history(&self, part: Option<&str>) -> Result<Vec<HistoryRecord>> {
        if let Some(name) = part {
            query_as::<_, HistoryRecord>(
                "SELECT timestamp, session_id, command, part_name, action, old_version, new_version, old_hash, new_hash, status, details
                 FROM history WHERE part_name = ? ORDER BY timestamp",
            )
            .bind(name)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to get history: {}", e))
            })
        } else {
            query_as::<_, HistoryRecord>(
                "SELECT timestamp, session_id, command, part_name, action, old_version, new_version, old_hash, new_hash, status, details
                 FROM history ORDER BY timestamp",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to get history: {}", e))
            })
        }
    }

    pub async fn update_history_status(&self, id: i64, status: HistoryStatus) -> Result<()> {
        query("UPDATE history SET status = ? WHERE id = ?")
            .bind(status)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to update history status: {}", e))
            })?;
        Ok(())
    }
}
