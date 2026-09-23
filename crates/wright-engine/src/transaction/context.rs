use std::path::Path;

use crate::error::Result;
use crate::transaction::fs_tx::{FsIntent, FsTransaction};
use wright_state::database::{HistoryAction, HistoryStatus, InstalledDb, SessionContext};

/// A filesystem transaction bound to a pending history row.
///
/// This is the single entry point every mutating operation (install, upgrade,
/// remove) uses. It pairs an [`FsTransaction`] — the one filesystem engine —
/// with the audit row that the operation will settle:
///
/// - `commit()` marks history `completed` and discards the backup store.
/// - `rollback()` restores the filesystem and marks the history `rolled_back`.
/// - Dropping without finalising restores the filesystem and leaves the
///   history row `pending`, for startup recovery to settle.
pub struct TransactionContext<'a> {
    db: &'a InstalledDb,
    fs: FsTransaction,
    tx_id: i64,
    part_name: String,
    finalized: bool,
}

impl<'a> TransactionContext<'a> {
    /// Begin a transaction for `part_name`, labelled with its history row id
    /// so the journal directory is unique per operation.
    pub async fn begin(
        db: &'a InstalledDb,
        root_dir: &Path,
        action: HistoryAction,
        part_name: &str,
        old_version: Option<&str>,
        new_version: Option<&str>,
        session: SessionContext,
        old_hash: Option<&str>,
        new_hash: Option<&str>,
    ) -> Result<Self> {
        let tx_id = db
            .record_history(
                &session.id,
                &session.command,
                part_name,
                action,
                old_version,
                new_version,
                old_hash,
                new_hash,
                HistoryStatus::Pending,
                None,
            )
            .await?;

        let intent = match action {
            HistoryAction::Install => FsIntent::install(part_name, new_hash),
            HistoryAction::Upgrade => FsIntent::upgrade(part_name, new_hash),
            // A rollback reverts toward the old hash, like a removal of the
            // new state; recovery compares against `old_hash`.
            HistoryAction::Remove | HistoryAction::Rollback => {
                FsIntent::remove(part_name, old_hash)
            }
        };
        let fs = FsTransaction::begin(root_dir, &tx_id.to_string(), &[intent])?;

        Ok(Self {
            db,
            fs,
            tx_id,
            part_name: part_name.to_string(),
            finalized: false,
        })
    }

    pub fn fs(&mut self) -> &mut FsTransaction {
        &mut self.fs
    }

    pub async fn commit(mut self) -> Result<()> {
        self.db
            .update_history_status(self.tx_id, HistoryStatus::Completed)
            .await?;
        self.fs.commit();
        self.finalized = true;
        Ok(())
    }

    pub async fn rollback(mut self) -> Result<()> {
        self.fs.rollback_blocking();
        self.db
            .update_history_status(self.tx_id, HistoryStatus::RolledBack)
            .await?;
        self.finalized = true;
        Ok(())
    }

    pub fn part_name(&self) -> &str {
        &self.part_name
    }

    pub fn db(&self) -> &InstalledDb {
        self.db
    }
}

impl<'a> Drop for TransactionContext<'a> {
    fn drop(&mut self) {
        // The FsTransaction's own Drop restores the filesystem; here we only
        // note that finalisation did not happen, leaving the history row
        // pending for startup recovery to settle.
        if !self.finalized {
            tracing::warn!(
                event = "transaction.dropped_unfinalized",
                part_name = %self.part_name,
                "Transaction dropped without commit/rollback; filesystem restored, history left pending"
            );
        }
    }
}
