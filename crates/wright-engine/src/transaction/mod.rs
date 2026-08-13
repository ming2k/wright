mod context;
mod deploy;
mod fs;
mod hooks;
mod remove;
pub mod rollback;
mod upgrade;
mod verify;

use crate::error::{Result, WrightError};
use std::path::PathBuf;
use wright_part::archive::PartInfo;
use wright_state::database::{InstalledDb, Origin};

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
/// independent sibling boundaries. `plan_source` is the raw `.PLANSRC`
/// member read back from the extracted archive (ADR-0033); when both it and
/// a plan checksum are present, the snapshot is recorded so the exact plan
/// content survives later source edits.
pub(super) async fn ensure_plan_registered(
    db: &InstalledDb,
    partinfo: &PartInfo,
    plan_source: Option<&str>,
) -> Result<i64> {
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

    let plan_id = db
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
        .await?;

    if let (Some(checksum), Some(source)) = (
        partinfo
            .provenance
            .as_ref()
            .and_then(|p| p.plan_checksum.as_deref()),
        plan_source,
    ) {
        db.insert_plan_snapshot(checksum, source).await?;
    }

    Ok(plan_id)
}

/// Guard against silently re-parenting a deployed part onto another plan.
///
/// Part names and plan names are independent namespaces: an archive for part
/// `x` built from plan `b` would otherwise take over the record of a part `x`
/// deployed from plan `a`, rewriting its `plan_id` and leaving plan `a`
/// behind as an empty shell. Re-parenting proceeds only when the incoming
/// archive declares the part in `replaces` (the documented rename/migration
/// path) or when the operation is forced — and the forced path warns loudly.
/// External placeholders (`wright provide`) are exempt: they carry no
/// provenance and exist to be replaced by a real deployment.
pub(super) async fn guard_plan_reparent(
    db: &InstalledDb,
    partinfo: &PartInfo,
    force: bool,
) -> Result<()> {
    let Some(installed) = db.get_part(&partinfo.name).await? else {
        return Ok(());
    };
    if installed.origin == Origin::External {
        return Ok(());
    }
    let installed_plan = db.get_plan_by_id(installed.plan_id).await?.ok_or_else(|| {
        WrightError::DeployError(format!(
            "plan for part '{}' not found in database",
            partinfo.name
        ))
    })?;
    if installed_plan.name == partinfo.plan.name {
        return Ok(());
    }
    if partinfo.replaces.iter().any(|name| name == &partinfo.name) {
        return Ok(());
    }
    if force {
        crate::cli_warn!(
            "part '{}' is deployed from plan '{}'; forcibly re-parenting it to plan '{}'",
            partinfo.name,
            installed_plan.name,
            partinfo.plan.name
        );
        return Ok(());
    }
    Err(WrightError::DeployError(format!(
        "part '{}' is deployed from plan '{}'; refusing to re-parent it to plan '{}'. \
         Remove it first (`wright remove {}`) or declare `replaces = [\"{}\"]` in plan '{}'.",
        partinfo.name,
        installed_plan.name,
        partinfo.plan.name,
        partinfo.name,
        partinfo.name,
        partinfo.plan.name,
    )))
}

pub mod dag;

#[cfg(test)]
mod tests {
    use super::*;
    use wright_part::archive::PlanMetadata;
    use wright_state::database::{NewPart, NewPlan};

    fn reparent_partinfo(plan_name: &str, replaces: &[&str]) -> PartInfo {
        PartInfo {
            name: "x".to_string(),
            build_date: "1970-01-01".to_string(),
            runtime_deps: Vec::new(),
            replaces: replaces.iter().map(|name| name.to_string()).collect(),
            conflicts: Vec::new(),
            backup_files: Vec::new(),
            plan: PlanMetadata {
                name: plan_name.to_string(),
                version: "1.0.0".to_string(),
                release: 1,
                epoch: 0,
                arch: "x86_64".to_string(),
            },
            provenance: None,
        }
    }

    async fn db_with_deployed_part(plan_name: &str, part_name: &str) -> InstalledDb {
        let db = InstalledDb::open_in_memory().await.unwrap();
        let plan_id = db
            .insert_plan(NewPlan {
                name: plan_name,
                version: "1.0.0",
                release: 1,
                epoch: 0,
                arch: "x86_64",
            })
            .await
            .unwrap();
        db.insert_part(NewPart {
            name: part_name,
            plan_id,
            ..Default::default()
        })
        .await
        .unwrap();
        db
    }

    #[tokio::test]
    async fn same_plan_redeploy_proceeds() {
        let db = db_with_deployed_part("a", "x").await;
        guard_plan_reparent(&db, &reparent_partinfo("a", &[]), false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn different_plan_without_replaces_or_force_errors() {
        let db = db_with_deployed_part("a", "x").await;
        let err = guard_plan_reparent(&db, &reparent_partinfo("b", &[]), false)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("plan 'a'") && msg.contains("plan 'b'"),
            "error should name both plans, got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn different_plan_with_replaces_proceeds() {
        let db = db_with_deployed_part("a", "x").await;
        guard_plan_reparent(&db, &reparent_partinfo("b", &["x"]), false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn different_plan_with_force_proceeds() {
        let db = db_with_deployed_part("a", "x").await;
        guard_plan_reparent(&db, &reparent_partinfo("b", &[]), true)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn external_placeholder_is_exempt() {
        // A `wright provide` placeholder carries no provenance; deploying the
        // real part over it is the intended un-provide flow.
        let db = InstalledDb::open_in_memory().await.unwrap();
        db.provide_part("x", "1.0").await.unwrap();
        guard_plan_reparent(&db, &reparent_partinfo("b", &[]), false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn uninstalled_part_proceeds() {
        let db = InstalledDb::open_in_memory().await.unwrap();
        guard_plan_reparent(&db, &reparent_partinfo("b", &[]), false)
            .await
            .unwrap();
    }
}
