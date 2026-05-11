use crate::database::{DeliveryStatus, DeliveryTransaction, InstalledDb, OpStatus, TransactionOp};
use crate::error::{Result, WrightError};
use sqlx::{query, query_as};

impl InstalledDb {
    /// Begin a new delivery transaction in PLANNING state.
    pub async fn begin_delivery(&self, command: &str) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();
        let res = query(
            "INSERT INTO delivery_transactions (command, status, created_at, updated_at)
             VALUES (?, 'planning', ?, ?)",
        )
        .bind(command)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            WrightError::DatabaseError(format!("failed to begin delivery transaction: {}", e))
        })?;
        Ok(res.last_insert_rowid())
    }

    /// Transition a delivery transaction to a new status.
    pub async fn set_delivery_status(&self, tx_id: i64, status: DeliveryStatus) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        query("UPDATE delivery_transactions SET status = ?, updated_at = ? WHERE id = ?")
            .bind(status)
            .bind(&now)
            .bind(tx_id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to update delivery status: {}", e))
            })?;
        Ok(())
    }

    /// Insert a single operation into the transaction ops table.
    pub async fn insert_transaction_op(
        &self,
        tx_id: i64,
        part_name: &str,
        part_hash: &str,
        action_type: &str,
        execution_order: i64,
        old_hash: Option<&str>,
    ) -> Result<i64> {
        let res = query(
            "INSERT INTO transaction_ops (transaction_id, part_name, part_hash, action_type, execution_order, status, old_hash)
             VALUES (?, ?, ?, ?, ?, 'pending', ?)",
        )
        .bind(tx_id)
        .bind(part_name)
        .bind(part_hash)
        .bind(action_type)
        .bind(execution_order)
        .bind(old_hash)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            WrightError::DatabaseError(format!("failed to insert transaction op: {}", e))
        })?;
        Ok(res.last_insert_rowid())
    }

    /// Insert multiple operations in a batch.
    pub async fn insert_transaction_ops(
        &self,
        tx_id: i64,
        ops: &[(String, String, String, i64, Option<String>)],
    ) -> Result<()> {
        for (part_name, part_hash, action_type, execution_order, old_hash) in ops {
            self.insert_transaction_op(
                tx_id,
                part_name,
                part_hash,
                action_type,
                *execution_order,
                old_hash.as_deref(),
            )
            .await?;
        }
        Ok(())
    }

    /// Update a single operation's status.
    pub async fn set_op_status(&self, op_id: i64, status: OpStatus) -> Result<()> {
        query("UPDATE transaction_ops SET status = ? WHERE id = ?")
            .bind(status)
            .bind(op_id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to update op status: {}", e))
            })?;
        Ok(())
    }

    /// Update an operation's status and error message.
    pub async fn set_op_failed(&self, op_id: i64, error_msg: &str) -> Result<()> {
        query("UPDATE transaction_ops SET status = 'failed', error_msg = ? WHERE id = ?")
            .bind(error_msg)
            .bind(op_id)
            .execute(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to set op failed: {}", e)))?;
        Ok(())
    }

    /// Find any delivery transaction that is not yet complete (leftover from a crash).
    pub async fn get_active_delivery(&self) -> Result<Option<DeliveryTransaction>> {
        let result: Option<DeliveryTransaction> = query_as(
            "SELECT id, command, status, created_at, updated_at
             FROM delivery_transactions
             WHERE status IN ('planning', 'ready', 'applying')
             ORDER BY id DESC
             LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            WrightError::DatabaseError(format!("failed to query active delivery: {}", e))
        })?;
        Ok(result)
    }

    /// Get all operations for a delivery transaction, ordered by execution_order.
    pub async fn get_ops_for_delivery(&self, tx_id: i64) -> Result<Vec<TransactionOp>> {
        let ops: Vec<TransactionOp> = query_as(
            "SELECT id, transaction_id, part_name, part_hash, action_type, execution_order, status, old_hash, error_msg
             FROM transaction_ops
             WHERE transaction_id = ?
             ORDER BY execution_order",
        )
        .bind(tx_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to query delivery ops: {}", e)))?;
        Ok(ops)
    }

    /// Reset an operation back to PENDING status (during crash recovery).
    pub async fn reset_op_to_pending(&self, op_id: i64) -> Result<()> {
        query("UPDATE transaction_ops SET status = 'pending', error_msg = NULL WHERE id = ?")
            .bind(op_id)
            .execute(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to reset op: {}", e)))?;
        Ok(())
    }

    /// Delete a delivery transaction and all its operations (called after successful completion or rollback).
    pub async fn cleanup_delivery(&self, tx_id: i64) -> Result<()> {
        // transaction_ops has a foreign key with ON DELETE CASCADE?
        // Let's check migration 015.
        query("DELETE FROM transaction_ops WHERE transaction_id = ?")
            .bind(tx_id)
            .execute(&self.pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to cleanup ops: {}", e)))?;

        query("DELETE FROM delivery_transactions WHERE id = ?")
            .bind(tx_id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to cleanup delivery: {}", e))
            })?;

        Ok(())
    }
}
