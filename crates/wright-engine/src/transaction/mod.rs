mod context;
mod deploy;
mod fs;
mod hooks;
mod remove;
pub mod rollback;
mod upgrade;
mod verify;

use crate::error::Result;
use std::path::PathBuf;
use std::time::Duration;
use tracing::debug;
use wright_part::archive::PartInfo;
use wright_state::database::InstalledDb;

pub use context::TransactionContext;
pub use deploy::{
    deploy_part, deploy_part_with_origin, deploy_parts, deploy_parts_with_explicit_targets,
};
pub use hooks::get_hook;
pub use remove::{
    cascade_remove_list, order_removal_batch, remove_part, remove_part_with_ignored_dependents,
};
pub use upgrade::upgrade_part;
pub use verify::verify_part;

/// Derive journal path from the database path.
pub(super) fn journal_path_from_db(db: &InstalledDb) -> Option<PathBuf> {
    db.db_path().map(|p| p.with_extension("journal"))
}

/// Replace conflicts and replaces rows for a part (used during upgrade).
pub(super) async fn self_replace_relations(
    db: &InstalledDb,
    part_id: i64,
    partinfo: &PartInfo,
) -> Result<()> {
    db.replace_conflicts(part_id, &partinfo.conflicts).await?;
    db.replace_replaces(part_id, &partinfo.replaces).await?;
    Ok(())
}

/// Persist the plan-level projection carried by a part archive.
///
/// The mapping belongs to the engine: archive and persistence types remain
/// independent sibling boundaries.
pub(super) async fn ensure_plan_registered(db: &InstalledDb, partinfo: &PartInfo) -> Result<i64> {
    let provenance =
        partinfo
            .provenance
            .as_ref()
            .map(|provenance| wright_state::database::NewPlanProvenance {
                plan_checksum: provenance.plan_checksum.as_deref(),
                source_checksums: &provenance.source_checksums,
                wright_version: &provenance.wright_version,
                isolation: &provenance.isolation,
            });

    Ok(db
        .ensure_plan_registered(wright_state::database::RegisterPlan {
            plan: wright_state::database::NewPlan {
                name: &partinfo.plan.name,
                version: &partinfo.plan.version,
                release: partinfo.plan.release,
                epoch: partinfo.plan.epoch,
                arch: &partinfo.plan.arch,
            },
            provenance,
        })
        .await?)
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
