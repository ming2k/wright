use super::InstalledDb;
use crate::error::{Result, WrightError};
use sqlx::query;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct ExecutionSession {
    pub session_hash: String,
    pub command_kind: String,
    pub task_session_hash: Option<String>,
    pub metadata_json: Option<String>,
}

impl InstalledDb {
    pub async fn ensure_execution_session(
        &self,
        session_hash: &str,
        command_kind: &str,
        task_session_hash: Option<&str>,
        metadata_json: Option<&str>,
    ) -> Result<()> {
        query(
            "INSERT INTO execution_sessions (session_hash, command_kind, task_session_hash, metadata_json)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(session_hash) DO UPDATE SET
               command_kind = excluded.command_kind,
               task_session_hash = excluded.task_session_hash,
               metadata_json = excluded.metadata_json",
        )
        .bind(session_hash)
        .bind(command_kind)
        .bind(task_session_hash)
        .bind(metadata_json)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            WrightError::DatabaseError(format!("failed to ensure execution session: {}", e))
        })?;
        Ok(())
    }

    pub async fn get_execution_session(
        &self,
        session_hash: &str,
    ) -> Result<Option<ExecutionSession>> {
        let row = query(
            "SELECT session_hash, command_kind, task_session_hash, metadata_json
             FROM execution_sessions
             WHERE session_hash = ?",
        )
        .bind(session_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            WrightError::DatabaseError(format!("failed to query execution session: {}", e))
        })?;

        let Some(row) = row else {
            return Ok(None);
        };

        use sqlx::Row;
        Ok(Some(ExecutionSession {
            session_hash: row
                .try_get(0)
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?,
            command_kind: row
                .try_get(1)
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?,
            task_session_hash: row
                .try_get(2)
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?,
            metadata_json: row
                .try_get(3)
                .map_err(|e| WrightError::DatabaseError(e.to_string()))?,
        }))
    }

    pub async fn ensure_execution_session_items(
        &self,
        session_hash: &str,
        item_keys: &[String],
    ) -> Result<()> {
        for item_key in item_keys {
            query(
                "INSERT OR IGNORE INTO execution_session_items (session_hash, item_key, status)
                 VALUES (?, ?, 'pending')",
            )
            .bind(session_hash)
            .bind(item_key)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!(
                    "failed to ensure execution session items: {}",
                    e
                ))
            })?;
        }
        Ok(())
    }

    pub async fn mark_execution_session_item_completed(
        &self,
        session_hash: &str,
        item_key: &str,
    ) -> Result<()> {
        query(
            "UPDATE execution_session_items
             SET status = 'completed'
             WHERE session_hash = ? AND item_key = ?",
        )
        .bind(session_hash)
        .bind(item_key)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            WrightError::DatabaseError(format!(
                "failed to mark execution session item completed: {}",
                e
            ))
        })?;
        Ok(())
    }

    pub async fn get_execution_session_completed_items(
        &self,
        session_hash: &str,
    ) -> Result<HashSet<String>> {
        let rows = query(
            "SELECT item_key
             FROM execution_session_items
             WHERE session_hash = ? AND status = 'completed'",
        )
        .bind(session_hash)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            WrightError::DatabaseError(format!("failed to query execution session items: {}", e))
        })?;

        let mut result = HashSet::new();
        for row in rows {
            use sqlx::Row;
            result.insert(
                row.try_get(0)
                    .map_err(|e| WrightError::DatabaseError(e.to_string()))?,
            );
        }
        Ok(result)
    }

    pub async fn clear_execution_session(&self, session_hash: &str) -> Result<()> {
        query("DELETE FROM execution_sessions WHERE session_hash = ?")
            .bind(session_hash)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to clear execution session: {}", e))
            })?;
        Ok(())
    }

    pub async fn clear_all_sessions(&self) -> Result<usize> {
        let row = query("SELECT COUNT(*) as count FROM execution_sessions")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to count execution sessions: {}", e))
            })?;

        use sqlx::Row;
        let count: i64 = row
            .try_get(0)
            .map_err(|e| WrightError::DatabaseError(e.to_string()))?;

        query("DELETE FROM execution_sessions")
            .execute(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to delete execution sessions: {}", e))
            })?;

        Ok(count as usize)
    }
}
