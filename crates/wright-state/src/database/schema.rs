//! Installed-state schema initialization.

use super::migrations::run_migrations;
use crate::error::Result;
use rusqlite::Connection;

pub fn init_db(conn: &mut Connection) -> Result<()> {
    run_migrations(conn)
}
