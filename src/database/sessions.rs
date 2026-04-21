use std::collections::HashSet;
use crate::error::{Result, WrightError};
use sqlx::query;
use super::InstalledDb;

impl InstalledDb {
    pub async fn create_session(&self, session_hash: &str, packages: &[String]) -> Result<()> {
        for part in packages {
            query("INSERT OR IGNORE INTO build_sessions (session_hash, package_name, status) VALUES (?, ?, 'pending')")
                .bind(session_hash)
                .bind(part)
            .execute(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to create session: {}", e)))?;
        }
        Ok(())
    }

    pub async fn mark_session_completed(&self, session_hash: &str, package_name: &str) -> Result<()> {
        query("UPDATE build_sessions SET status = 'completed' WHERE session_hash = ? AND package_name = ?")
            .bind(session_hash)
            .bind(package_name)
        .execute(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to mark session completed: {}", e)))?;
        Ok(())
    }

    pub async fn get_session_completed(&self, session_hash: &str) -> Result<HashSet<String>> {
        let rows = query("SELECT package_name FROM build_sessions WHERE session_hash = ? AND status = 'completed'")
            .bind(session_hash)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to query build session: {}", e)))?;

        let mut result = HashSet::new();
        for row in rows {
            use sqlx::Row;
            result.insert(row.try_get(0).map_err(|e| WrightError::DatabaseError(e.to_string()))?);
        }
        Ok(result)
    }

    pub async fn session_exists(&self, session_hash: &str) -> Result<bool> {
        let row = query("SELECT COUNT(*) as count FROM build_sessions WHERE session_hash = ?")
            .bind(session_hash)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to check session existence: {}", e)))?;
        
        use sqlx::Row;
        let count: i64 = row.try_get(0).map_err(|e| WrightError::DatabaseError(e.to_string()))?;
        Ok(count > 0)
    }

    pub async fn clear_session(&self, session_hash: &str) -> Result<()> {
        query("DELETE FROM build_sessions WHERE session_hash = ?")
            .bind(session_hash)
        .execute(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to clear session: {}", e)))?;
        Ok(())
    }

    pub async fn clear_all_sessions(&self) -> Result<usize> {
        let row = query("SELECT COUNT(DISTINCT session_hash) as count FROM build_sessions")
        .fetch_one(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to count sessions: {}", e)))?;

        use sqlx::Row;
        let count: i64 = row.try_get(0).map_err(|e| WrightError::DatabaseError(e.to_string()))?;

        query("DELETE FROM build_sessions")
            .execute(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to delete all sessions: {}", e)))?;
        
        Ok(count as usize)
    }
}
