use std::collections::HashSet;

use crate::error::Result;

use super::Database;

impl Database {
    pub fn create_session(&self, session_hash: &str, packages: &[String]) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "INSERT OR IGNORE INTO build_sessions (session_hash, package_name, status) VALUES (?1, ?2, 'pending')",
        )?;
        for pkg in packages {
            stmt.execute(rusqlite::params![session_hash, pkg])?;
        }
        Ok(())
    }

    pub fn mark_session_completed(&self, session_hash: &str, package_name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE build_sessions SET status = 'completed' WHERE session_hash = ?1 AND package_name = ?2",
            rusqlite::params![session_hash, package_name],
        )?;
        Ok(())
    }

    pub fn get_session_completed(&self, session_hash: &str) -> Result<HashSet<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT package_name FROM build_sessions WHERE session_hash = ?1 AND status = 'completed'",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![session_hash], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<std::result::Result<HashSet<_>, _>>()
            .map_err(|e| {
                crate::error::WrightError::DatabaseError(format!(
                    "failed to query build session: {}",
                    e
                ))
            })?;
        Ok(rows)
    }

    pub fn session_exists(&self, session_hash: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM build_sessions WHERE session_hash = ?1",
            rusqlite::params![session_hash],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn clear_session(&self, session_hash: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM build_sessions WHERE session_hash = ?1",
            rusqlite::params![session_hash],
        )?;
        Ok(())
    }

    pub fn clear_all_sessions(&self) -> Result<usize> {
        let count: usize = self.conn.query_row(
            "SELECT COUNT(DISTINCT session_hash) FROM build_sessions",
            [],
            |row| row.get(0),
        )?;
        self.conn.execute("DELETE FROM build_sessions", [])?;
        Ok(count)
    }
}
