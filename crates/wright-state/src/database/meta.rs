//! Database integrity, shadowing, and history queries.

use super::{HistoryAction, HistoryRecord, HistoryStatus, InstalledDb};
use crate::error::{Result, WrightError};
use rusqlite::params;
use std::path::Path;

impl InstalledDb {
    pub fn db_path(&self) -> Option<&Path> {
        self.db_path.as_deref()
    }

    pub async fn integrity_check(&self) -> Result<Vec<String>> {
        self.read(|conn| {
            let mut stmt = conn.prepare("PRAGMA integrity_check")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            let mut results = Vec::new();
            for r in rows {
                results.push(r?);
            }
            if results.len() == 1 && results[0] == "ok" {
                Ok(Vec::new())
            } else {
                Ok(results)
            }
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
