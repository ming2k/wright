//! SQLite persistence for delivery state-machine records.

use crate::database::{DeliveryStatus, DeliveryTransaction, InstalledDb, OpStatus, TransactionOp};
use crate::error::{Result, WrightError};
use rusqlite::params;

impl InstalledDb {
    /// Begin a new delivery transaction in PLANNING state.
    pub async fn begin_delivery(&self, command: &str) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();
        let command = command.to_string();
        self.write(move |conn| {
            conn.execute(
                "INSERT INTO delivery_transactions (command, status, created_at, updated_at)
                 VALUES (?1, 'planning', ?2, ?3)",
                params![command, now, now],
            )
            .map_err(|e| WrightError::context("failed to begin delivery transaction", e))?;
            Ok(conn.last_insert_rowid())
        })
        .await
    }

    /// Transition a delivery transaction to a new status.
    pub async fn set_delivery_status(&self, tx_id: i64, status: DeliveryStatus) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.write(move |conn| {
            conn.execute(
                "UPDATE delivery_transactions SET status = ?1, updated_at = ?2 WHERE id = ?3",
                params![status, now, tx_id],
            )
            .map_err(|e| WrightError::context("failed to update delivery status", e))?;
            Ok(())
        })
        .await
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
        let part_name = part_name.to_string();
        let part_hash = part_hash.to_string();
        let action_type = action_type.to_string();
        let old_hash = old_hash.map(|s| s.to_string());

        self.write(move |conn| {
            conn.execute(
                "INSERT INTO transaction_ops (transaction_id, part_name, part_hash, action_type, execution_order, status, old_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
                params![tx_id, part_name, part_hash, action_type, execution_order, old_hash],
            )
            .map_err(|e| WrightError::context("failed to insert transaction op", e))?;
            Ok(conn.last_insert_rowid())
        })
        .await
    }

    /// Insert multiple operations in a batch.
    pub async fn insert_transaction_ops(
        &self,
        tx_id: i64,
        ops: &[(String, String, String, i64, Option<String>)],
    ) -> Result<()> {
        let ops = ops.to_vec();
        self.write(move |conn| {
            let mut stmt = conn.prepare(
                "INSERT INTO transaction_ops (transaction_id, part_name, part_hash, action_type, execution_order, status, old_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
            )?;
            for (part_name, part_hash, action_type, execution_order, old_hash) in &ops {
                stmt.execute(params![
                    tx_id,
                    part_name,
                    part_hash,
                    action_type,
                    execution_order,
                    old_hash,
                ])
                .map_err(|e| WrightError::context("failed to insert transaction op", e))?;
            }
            Ok(())
        })
        .await
    }

    /// Update a single operation's status.
    pub async fn set_op_status(&self, op_id: i64, status: OpStatus) -> Result<()> {
        self.write(move |conn| {
            conn.execute(
                "UPDATE transaction_ops SET status = ?1 WHERE id = ?2",
                params![status, op_id],
            )
            .map_err(|e| WrightError::context("failed to update op status", e))?;
            Ok(())
        })
        .await
    }

    /// Update an operation's status and error message.
    pub async fn set_op_failed(&self, op_id: i64, error_msg: &str) -> Result<()> {
        let error_msg = error_msg.to_string();
        self.write(move |conn| {
            conn.execute(
                "UPDATE transaction_ops SET status = 'failed', error_msg = ?1 WHERE id = ?2",
                params![error_msg, op_id],
            )
            .map_err(|e| WrightError::context("failed to set op failed", e))?;
            Ok(())
        })
        .await
    }

    /// Find any delivery transaction that is not yet complete (leftover from a crash).
    pub async fn get_active_delivery(&self) -> Result<Option<DeliveryTransaction>> {
        self.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, command, status, created_at, updated_at
                 FROM delivery_transactions
                 WHERE status IN ('planning', 'ready', 'applying')
                 ORDER BY id DESC
                 LIMIT 1",
            )?;
            let mut rows = stmt.query([])?;
            if let Some(row) = rows.next()? {
                Ok(Some(DeliveryTransaction::from_row(row)?))
            } else {
                Ok(None)
            }
        })
        .await
    }

    /// Get all operations for a delivery transaction, ordered by execution_order.
    pub async fn get_ops_for_delivery(&self, tx_id: i64) -> Result<Vec<TransactionOp>> {
        self.read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, transaction_id, part_name, part_hash, action_type, execution_order, status, old_hash, error_msg
                 FROM transaction_ops
                 WHERE transaction_id = ?1
                 ORDER BY execution_order",
            )?;
            let rows = stmt.query_map(params![tx_id], TransactionOp::from_row)?;
            let mut ops = Vec::new();
            for r in rows {
                ops.push(r?);
            }
            Ok(ops)
        })
        .await
    }

    /// Reset an operation back to PENDING status (during crash recovery).
    pub async fn reset_op_to_pending(&self, op_id: i64) -> Result<()> {
        self.write(move |conn| {
            conn.execute(
                "UPDATE transaction_ops SET status = 'pending', error_msg = NULL WHERE id = ?1",
                params![op_id],
            )
            .map_err(|e| WrightError::context("failed to reset op", e))?;
            Ok(())
        })
        .await
    }

    /// Delete a delivery transaction and all its operations (called after successful completion or rollback).
    pub async fn cleanup_delivery(&self, tx_id: i64) -> Result<()> {
        self.write(move |conn| {
            conn.execute("DELETE FROM transaction_ops WHERE transaction_id = ?1", params![tx_id])
                .map_err(|e| WrightError::context("failed to cleanup ops", e))?;

            conn.execute("DELETE FROM delivery_transactions WHERE id = ?1", params![tx_id])
                .map_err(|e| WrightError::context("failed to cleanup delivery", e))?;

            Ok(())
        })
        .await
    }
}
