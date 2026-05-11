//! Delivery state machine — two-phase commit for system-mutation operations.
//!
//! # Architecture
//!
//! Layer 1: Forge & Seal use CAS (Content-Addressed Storage).
//!   The file system is the source of truth.  After resolve determines what
//!   needs to be built, if a `.part` with the correct closure fingerprint
//!   already exists on disk, forge+seal are skipped.
//!
//! Layer 2: Deploy uses a Write-Ahead Log (WAL) in SQLite.
//!   Two tables — `delivery_transactions` and `transaction_ops` — form a
//!   Write-Ahead Log that survives crashes.  On restart, incomplete
//!   transactions are detected, mid-flight operations are cleaned up, and
//!   deployment resumes from the first PENDING op.
//!
//! # State machine
//!
//! Delivery:  PLANNING → READY → APPLYING → COMPLETED
//!   (Planning encompasses resolve; READY means forge+seal complete.)
//!   Op:        PENDING → EXTRACTING → HOOKS_RUNNING → DONE
//!
//!   On failure: any state → ROLLED_BACK / FAILED

use tracing::{info, warn};

use crate::database::{DeliveryStatus, DeliveryTransaction, InstalledDb, OpStatus, TransactionOp};
use crate::error::{Result, WrightError};

pub mod store;

/// Begin a new delivery transaction in PLANNING state.
pub async fn begin_delivery(db: &InstalledDb, command: &str) -> Result<i64> {
    db.begin_delivery(command).await
}

/// Transition the delivery to READY — resolve and forge+seal pre-deploy work is done.
pub async fn delivery_ready(db: &InstalledDb, tx_id: i64) -> Result<()> {
    db.set_delivery_status(tx_id, DeliveryStatus::Ready).await
}

/// Transition the delivery to APPLYING — the system is about to be mutated.
pub async fn begin_applying(db: &InstalledDb, tx_id: i64) -> Result<()> {
    db.set_delivery_status(tx_id, DeliveryStatus::Applying)
        .await
}

/// Mark the delivery as COMPLETED — all operations finished successfully.
pub async fn complete_delivery(db: &InstalledDb, tx_id: i64) -> Result<()> {
    db.set_delivery_status(tx_id, DeliveryStatus::Completed)
        .await
}

/// Mark the delivery as ROLLED_BACK after a failure.
pub async fn rollback_delivery(db: &InstalledDb, tx_id: i64) -> Result<()> {
    db.set_delivery_status(tx_id, DeliveryStatus::RolledBack)
        .await
}

/// Cleanup a delivery transaction after it has been fully processed (committed or rolled back).
pub async fn cleanup_delivery(db: &InstalledDb, tx_id: i64) -> Result<()> {
    db.cleanup_delivery(tx_id).await
}

/// Register the planned operations for a delivery transaction.
///
/// `ops` is a list of `(part_name, part_hash, action_type, execution_order, old_hash)`.
pub async fn register_ops(
    db: &InstalledDb,
    tx_id: i64,
    ops: &[(String, String, String, i64, Option<String>)],
) -> Result<()> {
    db.insert_transaction_ops(tx_id, ops).await
}

/// Update a single operation to EXTRACTING (about to copy files).
pub async fn op_extracting(db: &InstalledDb, op_id: i64) -> Result<()> {
    db.set_op_status(op_id, OpStatus::Extracting).await
}

/// Update a single operation to HOOKS_RUNNING (about to run pre/post scripts).
pub async fn op_hooks_running(db: &InstalledDb, op_id: i64) -> Result<()> {
    db.set_op_status(op_id, OpStatus::HooksRunning).await
}

/// Update a single operation to DONE.
pub async fn op_done(db: &InstalledDb, op_id: i64) -> Result<()> {
    db.set_op_status(op_id, OpStatus::Done).await
}

/// Mark an operation as FAILED.
pub async fn op_failed(db: &InstalledDb, op_id: i64, error_msg: &str) -> Result<()> {
    db.set_op_failed(op_id, error_msg).await
}

/// Crash recovery — called once at process startup.
///
/// 1. Look for any delivery in PLANNING, READY, or APPLYING state.
/// 2. If found in PLANNING: mark as ROLLED_BACK (never reached system mutation).
/// 3. If found in READY or APPLYING: resume from the first PENDING operation,
///    after cleaning up any mid-flight EXTRACTING / HOOKS_RUNNING ops.
pub async fn recover_if_needed(db: &InstalledDb) -> Result<bool> {
    let Some(active) = db.get_active_delivery().await? else {
        return Ok(false);
    };

    info!(
        "Found incomplete delivery transaction {} (status: {:?}, command: {})",
        active.id, active.status, active.command
    );

    match active.status {
        DeliveryStatus::Planning => {
            // Never reached system mutation — safe to discard.
            info!("Delivery was still in PLANNING phase — discarding");
            db.set_delivery_status(active.id, DeliveryStatus::RolledBack)
                .await?;
        }
        DeliveryStatus::Ready | DeliveryStatus::Applying => {
            recover_from_applying(db, &active).await?;
        }
        _ => {}
    }

    Ok(true)
}

/// Resume or clean up a partially-completed delivery.
async fn recover_from_applying(db: &InstalledDb, tx: &DeliveryTransaction) -> Result<()> {
    let ops = db.get_ops_for_delivery(tx.id).await?;

    if ops.is_empty() {
        info!("No operations found for transaction {} — completing", tx.id);
        db.set_delivery_status(tx.id, DeliveryStatus::Completed)
            .await?;
        return Ok(());
    }

    // Audit each operation and decide what to do.
    let mut done_ops: Vec<String> = Vec::new();
    let mut midflight_ops: Vec<(&TransactionOp, OpStatus)> = Vec::new();
    let mut pending_exists = false;

    for op in &ops {
        match op.status {
            OpStatus::Done => {
                done_ops.push(op.part_name.clone());
            }
            OpStatus::Extracting | OpStatus::HooksRunning => {
                midflight_ops.push((op, op.status));
            }
            OpStatus::Pending => {
                pending_exists = true;
            }
            OpStatus::Failed => {
                // A previously-failed op means the user likely saw an error
                // before the crash.  Leave the whole thing as-is for them to
                // manually resolve, or re-run the command.
                warn!(
                    "Delivery {} has a FAILED op ({}); aborting recovery — re-run the command",
                    tx.id, op.part_name
                );
                return Err(WrightError::DeployError(format!(
                    "delivery transaction {} has a failed op ({}); re-run the install/upgrade command",
                    tx.id, op.part_name
                )));
            }
        }
    }

    // Clean up mid-flight operations: the filesystem state for these is
    // undefined — their files were partially extracted and need to be
    // treated as "not yet done".  The old per-op rollback journal (from
    // TransactionContext) will have already cleaned up any individual file
    // mutations on drop.  We just reset them to PENDING so they will be
    // retried.
    for (op, old_status) in &midflight_ops {
        warn!(
            "Cleaning up mid-flight op {} (was {:?}) — resetting to pending",
            op.part_name, old_status
        );
        db.reset_op_to_pending(op.id).await?;
    }

    let midflight_names: Vec<_> = midflight_ops
        .iter()
        .map(|(op, _)| op.part_name.as_str())
        .collect();
    let done_names: Vec<_> = done_ops.iter().map(|s| s.as_str()).collect();

    info!(
        "Delivery {} recovery: {} done ({}), {} mid-flight reset to pending ({}), {} remaining pending. \
         Re-run your install/upgrade command to continue.",
        tx.id,
        done_ops.len(),
        done_names.join(", "),
        midflight_names.len(),
        midflight_names.join(", "),
        if pending_exists { "some" } else { "no" },
    );

    // Mark the transaction as ROLLED_BACK so the user knows to re-run.
    // The re-run will pick up from where we left off (CAS store will have
    // the pre-built parts, and the WAL will be re-created fresh).
    db.set_delivery_status(tx.id, DeliveryStatus::RolledBack)
        .await?;

    Ok(())
}
