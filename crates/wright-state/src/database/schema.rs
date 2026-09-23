//! Installed-state schema initialization.

use super::migrations::run_migrations;
use crate::error::Result;
use rusqlite::Connection;
use std::path::Path;

/// Initialize the schema, applying pending migrations. `db_path`, when known,
/// is where the pre-migration snapshot is written (ADR-0043).
pub fn init_db(conn: &mut Connection, db_path: Option<&Path>) -> Result<()> {
    run_migrations(conn, db_path)
}
