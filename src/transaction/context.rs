use crate::database::{HistoryAction, HistoryStatus, InstalledDb, SessionContext};
use crate::error::Result;
use crate::transaction::rollback::RollbackState;

/// Unified transaction context coordinating filesystem rollback and audit logging.
///
/// # Design Principles
///
/// - All filesystem mutations are recorded via `rollback_state()`.
/// - On success, call `commit()` to atomically update the database and clean up rollback state.
/// - On failure, call `rollback()` to restore filesystem state and record the failure.
/// - If dropped without explicit finalization, the filesystem is automatically rolled back
///   (the database status is NOT updated in Drop to avoid async-in-Drop issues; callers
///   should use `rollback()` explicitly on the error path).
pub struct TransactionContext<'a> {
    db: &'a InstalledDb,
    rollback: RollbackState,
    tx_id: i64,
    part_name: String,
    finalized: bool,
}

impl<'a> TransactionContext<'a> {
    pub async fn begin(
        db: &'a InstalledDb,
        action: HistoryAction,
        part_name: &str,
        old_version: Option<&str>,
        new_version: Option<&str>,
        session: SessionContext,
        old_hash: Option<&str>,
        new_hash: Option<&str>,
    ) -> Result<Self> {
        let rollback = match super::journal_path_from_db(db) {
            Some(jp) => RollbackState::with_journal(jp),
            None => RollbackState::new(),
        };

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

        Ok(Self {
            db,
            rollback,
            tx_id,
            part_name: part_name.to_string(),
            finalized: false,
        })
    }

    pub fn rollback_state(&mut self) -> &mut RollbackState {
        &mut self.rollback
    }

    pub async fn commit(mut self) -> Result<()> {
        self.db
            .update_history_status(self.tx_id, HistoryStatus::Completed)
            .await?;
        self.rollback.commit();
        self.finalized = true;
        Ok(())
    }

    pub async fn rollback(mut self) -> Result<()> {
        self.rollback.rollback();
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
        if !self.finalized {
            self.rollback.rollback();
        }
    }
}
