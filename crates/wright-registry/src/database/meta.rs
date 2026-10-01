//! Database integrity, shadowing, and history queries.

use super::{HistoryAction, HistoryRecord, HistoryStatus, InstalledDb, ReadOnlyDb};
use crate::error::{Result, WrightError};
use rusqlite::params;

impl ReadOnlyDb {
    /// Structural plus referential integrity. `PRAGMA integrity_check` catches
    /// corruption of the b-tree and indexes; `PRAGMA foreign_key_check` catches
    /// the subtler damage a partially-applied migration leaves — rows whose
    /// foreign keys point at parents that no longer exist (ADR-0043).
    pub async fn integrity_check(&self) -> Result<Vec<String>> {
        self.read(|conn| {
            let mut results = Vec::new();

            let mut stmt = conn.prepare("PRAGMA integrity_check")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            let mut integrity = Vec::new();
            for r in rows {
                integrity.push(r?);
            }
            if !(integrity.len() == 1 && integrity[0] == "ok") {
                results.extend(integrity);
            }
            drop(stmt);

            let mut fk_stmt = conn.prepare("PRAGMA foreign_key_check")?;
            let fk_rows = fk_stmt.query_map([], |r| {
                let table: String = r.get(0)?;
                let rowid: Option<i64> = r.get(1)?;
                let parent: String = r.get(2)?;
                Ok(format!(
                    "foreign key violation: {}.rowid={} references missing row in {}",
                    table,
                    rowid
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "<null>".to_string()),
                    parent
                ))
            })?;
            for r in fk_rows {
                results.push(r?);
            }

            Ok(results)
        })
        .await
    }

    pub async fn get_shadowed_conflicts(&self) -> Result<Vec<String>> {
        self.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT s.path, p1.name as original, p2.name as shadower 
                 FROM shadowed_files s
                 JOIN parts p1 ON s.original_owner_id = p1.id
                 JOIN parts p2 ON s.shadowed_by_id = p2.id",
            )?;
            let rows = stmt.query_map([], |row| {
                let path: String = row.get(0)?;
                let original: String = row.get(1)?;
                let shadower: String = row.get(2)?;
                Ok(format!(
                    "Path '{}' (owned by {}) is shadowed by {}",
                    path, original, shadower
                ))
            })?;
            let mut result = Vec::new();
            for r in rows {
                result.push(r?);
            }
            Ok(result)
        })
        .await
    }

    /// Check for archive-level protocol metadata files mistakenly recorded
    /// into the `files` or `shadowed_files` tables.
    pub async fn get_leaked_metadata_conflicts(
        &self,
        metadata_files: &[&str],
    ) -> Result<Vec<String>> {
        let targets: Vec<String> = metadata_files
            .iter()
            .map(|name| format!("/{}", name.trim_start_matches('/')))
            .collect();
        self.read(move |conn| {
            let mut results = Vec::new();
            for target in &targets {
                let mut stmt = conn.prepare(
                    "SELECT p.name, f.path FROM files f
                     JOIN parts p ON f.part_id = p.id
                     WHERE f.path = ?1",
                )?;
                let rows = stmt.query_map(rusqlite::params![target], |row| {
                    let part: String = row.get(0)?;
                    let path: String = row.get(1)?;
                    Ok(format!(
                        "Archive metadata '{}' leaked into installed part '{}'",
                        path, part
                    ))
                })?;
                for r in rows {
                    results.push(r?);
                }

                let mut stmt_shadow = conn.prepare(
                    "SELECT path FROM shadowed_files WHERE path = ?1",
                )?;
                let rows_shadow = stmt_shadow.query_map(rusqlite::params![target], |row| {
                    let path: String = row.get(0)?;
                    Ok(format!(
                        "Archive metadata '{}' present in shadowed conflicts",
                        path
                    ))
                })?;
                for r in rows_shadow {
                    results.push(r?);
                }
            }
            Ok(results)
        })
        .await
    }

    pub async fn get_history(&self, part: Option<&str>) -> Result<Vec<HistoryRecord>> {
        let part = part.map(|s| s.to_string());
        self.read(move |conn| {
            let mut records = Vec::new();
            if let Some(ref name) = part {
                let mut stmt = conn.prepare(
                    "SELECT timestamp, session_id, command, part_name, action, old_version, new_version, old_hash, new_hash, status, details
                     FROM history WHERE part_name = ?1 ORDER BY timestamp",
                )?;
                let rows = stmt.query_map(params![name], HistoryRecord::from_row)?;
                for r in rows {
                    records.push(r?);
                }
            } else {
                let mut stmt = conn.prepare(
                    "SELECT timestamp, session_id, command, part_name, action, old_version, new_version, old_hash, new_hash, status, details
                     FROM history ORDER BY timestamp",
                )?;
                let rows = stmt.query_map([], HistoryRecord::from_row)?;
                for r in rows {
                    records.push(r?);
                }
            }
            Ok(records)
        })
        .await
    }
}

impl InstalledDb {
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
        let session_id = session_id.to_string();
        let command = command.to_string();
        let part_name = part_name.to_string();
        let old_version = old_version.map(|s| s.to_string());
        let new_version = new_version.map(|s| s.to_string());
        let old_hash = old_hash.map(|s| s.to_string());
        let new_hash = new_hash.map(|s| s.to_string());
        let details = details.map(|s| s.to_string());

        self.write(move |conn| {
            conn.execute(
                "INSERT INTO history (session_id, command, part_name, action, old_version, new_version, old_hash, new_hash, status, details)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    session_id,
                    command,
                    part_name,
                    action,
                    old_version,
                    new_version,
                    old_hash,
                    new_hash,
                    status,
                    details,
                ],
            )
            .map_err(|e| WrightError::context("failed to record history", e))?;
            Ok(conn.last_insert_rowid())
        })
        .await
    }

    /// Commit a removal batch: delete every named part and settle the matching
    /// pending history rows — in a single SQL transaction.
    ///
    /// This is the commit point of a removal. The filesystem work happens
    /// first and is journaled; this call flips the registry, and the registry
    /// is what makes the journal's copy of events either "to be undone" (parts
    /// still present) or "committed, discard the backups". Doing both the
    /// delete and the history settlement in one transaction means a removal
    /// can never half-commit: either every part is gone and every history row
    /// reads `completed`, or nothing changed.
    ///
    /// Plan rows left with no parts are removed. Returns the names actually
    /// deleted, so a caller racing an external change sees what it committed.
    pub async fn commit_removal_batch(&self, names: &[String]) -> Result<Vec<String>> {
        let names = names.to_vec();
        self.write(move |conn| {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| WrightError::context("failed to begin removal commit", e))?;

            let mut removed = Vec::new();
            let mut touched_plans: Vec<i64> = Vec::new();

            for name in &names {
                let plan_id: Option<i64> = tx
                    .query_row(
                        "SELECT plan_id FROM parts WHERE name = ?1",
                        params![name],
                        |r| r.get(0),
                    )
                    .ok();

                let affected = tx
                    .execute("DELETE FROM parts WHERE name = ?1", params![name])
                    .map_err(|e| WrightError::context("failed to delete part", e))?;

                if affected == 0 {
                    // Already gone (external change): still settle history so
                    // the audit trail does not keep a pending row forever.
                    continue;
                }
                removed.push(name.clone());
                if let Some(plan_id) = plan_id
                    && !touched_plans.contains(&plan_id)
                {
                    touched_plans.push(plan_id);
                }
            }

            for plan_id in touched_plans {
                let remaining: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM parts WHERE plan_id = ?1",
                        params![plan_id],
                        |r| r.get(0),
                    )
                    .map_err(|e| WrightError::context("failed to count remaining parts", e))?;
                if remaining == 0 {
                    tx.execute("DELETE FROM plans WHERE id = ?1", params![plan_id])
                        .map_err(|e| WrightError::context("failed to delete empty plan", e))?;
                }
            }

            // Settle the pending history rows for the parts we removed.
            for name in &removed {
                tx.execute(
                    "UPDATE history SET status = ?1
                     WHERE part_name = ?2 AND action = ?3 AND status = ?4",
                    params![
                        HistoryStatus::Completed,
                        name,
                        HistoryAction::Remove,
                        HistoryStatus::Pending,
                    ],
                )
                .map_err(|e| WrightError::context("failed to settle removal history", e))?;
            }

            tx.commit()
                .map_err(|e| WrightError::context("failed to commit removal batch", e))?;
            Ok(removed)
        })
        .await
    }

    /// Settle every pending history row for a session as `rolled_back`.
    ///
    /// Called when an operation aborts before its registry commit, so the
    /// audit trail never keeps a `pending` row that will never resolve. Scoped
    /// by session id because a session is one user invocation; a batch that
    /// aborts mid-way has every one of its pending rows rolled back together.
    pub async fn rollback_history_session(&self, session_id: &str) -> Result<u64> {
        let session_id = session_id.to_string();
        self.write(move |conn| {
            let affected = conn
                .execute(
                    "UPDATE history SET status = ?1 WHERE session_id = ?2 AND status = ?3",
                    params![
                        HistoryStatus::RolledBack,
                        session_id,
                        HistoryStatus::Pending,
                    ],
                )
                .map_err(|e| WrightError::context("failed to roll back history session", e))?;
            Ok(affected as u64)
        })
        .await
    }

    /// Record the config files a removal deliberately left on disk, so the
    /// residue is discoverable rather than silently orphaned. Details land on
    /// the part's removal history row as a JSON list.
    pub async fn record_removal_residue(
        &self,
        session_id: &str,
        part_name: &str,
        paths: &[String],
    ) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let session_id = session_id.to_string();
        let part_name = part_name.to_string();
        let details = serde_json::to_string(paths)
            .map_err(|e| WrightError::context("failed to serialize removal residue", e))?;
        self.write(move |conn| {
            conn.execute(
                "UPDATE history SET details = ?1
                 WHERE session_id = ?2 AND part_name = ?3 AND action = ?4
                   AND status = ?5",
                params![
                    details,
                    session_id,
                    part_name,
                    HistoryAction::Remove,
                    HistoryStatus::Completed,
                ],
            )
            .map_err(|e| WrightError::context("failed to record removal residue", e))?;
            Ok(())
        })
        .await
    }

    pub async fn update_history_status(&self, id: i64, status: HistoryStatus) -> Result<()> {
        self.write(move |conn| {
            conn.execute(
                "UPDATE history SET status = ?1 WHERE id = ?2",
                params![status, id],
            )
            .map_err(|e| WrightError::context("failed to update history status", e))?;
            Ok(())
        })
        .await
    }
}
