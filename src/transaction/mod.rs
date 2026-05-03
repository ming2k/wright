mod fs;
mod hooks;
mod install;
mod remove;
pub mod rollback;
mod upgrade;
mod verify;

use crate::database::InstalledDb;
use crate::error::{Result, WrightError};
use crate::part::part::PartInfo;
use std::path::PathBuf;
use std::time::Duration;
use tracing::debug;

pub use hooks::get_hook;
pub use install::{
    install_part, install_part_with_origin, install_parts, install_parts_with_explicit_targets,
    install_parts_with_explicit_targets_and_plan_map, install_parts_with_plan_map,
};
pub use remove::{
    cascade_remove_list, order_removal_batch, remove_part, remove_part_with_ignored_dependents,
};
pub use upgrade::upgrade_part;
pub use verify::verify_part;

/// Compacts a file path for cleaner logging by replacing middle directories with `...`
/// if the path exceeds a reasonable length threshold.
pub fn compact_path(path: &str) -> String {
    if path.len() <= 45 {
        return path.to_string();
    }
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 4 {
        return path.to_string();
    }
    let n = parts.len();
    format!("/{}/.../{}/{}", parts[0], parts[n - 2], parts[n - 1])
}

/// Derive journal path from the database path.
pub(super) fn journal_path_from_db(db: &InstalledDb) -> Option<PathBuf> {
    db.db_path().map(|p| p.with_extension("journal"))
}

/// Replace provides, conflicts, and replaces rows for a part (used during upgrade).
pub(super) async fn self_replace_provides_conflicts(
    db: &InstalledDb,
    part_id: i64,
    partinfo: &PartInfo,
) -> Result<()> {
    sqlx::query("DELETE FROM provides WHERE part_id = ?")
        .bind(part_id)
        .execute(&db.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to delete old provides: {}", e)))?;
    sqlx::query("DELETE FROM conflicts WHERE part_id = ?")
        .bind(part_id)
        .execute(&db.pool)
        .await
        .map_err(|e| {
            WrightError::DatabaseError(format!("failed to delete old conflicts: {}", e))
        })?;
    sqlx::query("DELETE FROM replaces WHERE part_id = ?")
        .bind(part_id)
        .execute(&db.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to delete old replaces: {}", e)))?;

    if !partinfo.provides.is_empty() {
        db.insert_provides(part_id, &partinfo.provides).await?;
    }
    if !partinfo.conflicts.is_empty() {
        db.insert_conflicts(part_id, &partinfo.conflicts).await?;
    }
    if !partinfo.replaces.is_empty() {
        db.insert_replaces(part_id, &partinfo.replaces).await?;
    }
    Ok(())
}

pub(super) fn log_debug_timing(operation: &str, part_name: &str, phase: &str, elapsed: Duration) {
    debug!(
        "{} {}: {} completed in {:.3}s",
        operation,
        part_name,
        phase,
        elapsed.as_secs_f64()
    );
}

pub mod dag;
