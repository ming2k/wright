use rusqlite::Connection;
use std::path::Path;

use crate::error::Result;

use super::{row_to_transaction, InstalledDb, TransactionRecord};

impl InstalledDb {
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn db_path(&self) -> Option<&Path> {
        self.db_path.as_deref()
    }

    pub fn integrity_check(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("PRAGMA integrity_check")?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        if rows.len() == 1 && rows[0] == "ok" {
            Ok(Vec::new())
        } else {
            Ok(rows)
        }
    }

    pub fn get_shadowed_conflicts(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.path, p1.name as original, p2.name as shadower 
             FROM shadowed_files s
             JOIN parts p1 ON s.original_owner_id = p1.id
             JOIN parts p2 ON s.shadowed_by_id = p2.id",
        )?;

        let rows = stmt
            .query_map([], |row| {
                Ok(format!(
                    "Path '{}' (owned by {}) is shadowed by {}",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    pub fn record_transaction(
        &self,
        operation: &str,
        part_name: &str,
        old_version: Option<&str>,
        new_version: Option<&str>,
        status: &str,
        backup_path: Option<&str>,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO transactions (operation, part_name, old_version, new_version, status, backup_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![operation, part_name, old_version, new_version, status, backup_path],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_history(&self, part: Option<&str>) -> Result<Vec<TransactionRecord>> {
        let mut records = Vec::new();
        if let Some(name) = part {
            let mut stmt = self.conn.prepare(
                "SELECT timestamp, operation, part_name, old_version, new_version, status
                 FROM transactions WHERE part_name = ?1 ORDER BY timestamp",
            )?;
            let rows = stmt.query_map(rusqlite::params![name], row_to_transaction)?;
            for row in rows {
                records.push(row?);
            }
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT timestamp, operation, part_name, old_version, new_version, status
                 FROM transactions ORDER BY timestamp",
            )?;
            let rows = stmt.query_map([], row_to_transaction)?;
            for row in rows {
                records.push(row?);
            }
        }
        Ok(records)
    }

    pub fn update_transaction_status(&self, id: i64, status: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE transactions SET status = ?1 WHERE id = ?2",
            rusqlite::params![status, id],
        )?;
        Ok(())
    }
}
