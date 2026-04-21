use super::{InstalledDb, TransactionRecord};
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

    pub async fn record_transaction(
        &self,
        operation: &str,
        part_name: &str,
        old_version: Option<&str>,
        new_version: Option<&str>,
        status: &str,
        backup_path: Option<&str>,
    ) -> Result<i64> {
        let res = query(
            "INSERT INTO transactions (operation, part_name, old_version, new_version, status, backup_path)
             VALUES (?, ?, ?, ?, ?, ?)")
            .bind(operation)
            .bind(part_name)
            .bind(old_version)
            .bind(new_version)
            .bind(status)
            .bind(backup_path)
        .execute(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to record transaction: {}", e)))?;

        Ok(res.last_insert_rowid())
    }

    pub async fn get_history(&self, part: Option<&str>) -> Result<Vec<TransactionRecord>> {
        if let Some(name) = part {
            query_as::<_, TransactionRecord>(
                "SELECT timestamp, operation, part_name, old_version, new_version, status
                 FROM transactions WHERE part_name = ? ORDER BY timestamp",
            )
            .bind(name)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to get transaction history: {}", e))
            })
        } else {
            query_as::<_, TransactionRecord>(
                "SELECT timestamp, operation, part_name, old_version, new_version, status
                 FROM transactions ORDER BY timestamp",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to get transaction history: {}", e))
            })
        }
    }

    pub async fn update_transaction_status(&self, id: i64, status: &str) -> Result<()> {
        query("UPDATE transactions SET status = ? WHERE id = ?")
            .bind(status)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to update transaction status: {}", e))
            })?;
        Ok(())
    }
}
