use std::collections::HashSet;

use crate::error::{Result, WrightError};
use crate::identify::Identifier;
use crate::transaction::{self, PartRef, RemovalBatch};
use wright_registry::database::{InstalledDb, SessionContext};

/// Execute `wright remove` for one or more targets.
///
/// The whole batch is planned up front (targets expanded, dependents and
/// orphans resolved, order computed, blockers detected) before any file is
/// touched. It then runs as one delivery transaction:
///
/// - every part's filesystem teardown is journaled under a single removal
///   transaction,
/// - the registry is flipped for the entire batch in one SQL transaction,
/// - a failure anywhere restores every journaled file and leaves the registry
///   untouched.
///
/// Dependents that are not themselves in the batch block the removal unless
/// `--force`. `--recursive` pulls dependents into the batch; `--cascade` pulls
/// in orphaned dependencies.
pub async fn execute_remove(
    db: &InstalledDb,
    parts: &[&str],
    force: bool,
    recursive: bool,
    cascade: bool,
    dry_run: bool,
    root_dir: &std::path::Path,
) -> Result<()> {
    let plan = plan_removal(db, parts, recursive, cascade).await?;

    if plan.targets.is_empty() {
        return Err(WrightError::ValidationError(
            "no deployed parts matched the given targets".to_string(),
        ));
    }

    if dry_run {
        crate::outln!("[dry-run] remove -> {}", root_dir.display());
        crate::outln!("[dry-run] would remove {} part(s):", plan.targets.len());
        for name in &plan.targets {
            crate::outln!("  {}", name);
        }
        return Ok(());
    }

    let command_str = format!("remove {}", plan.requested.join(" "));
    let session = SessionContext {
        id: format!(
            "{:x}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ),
        command: command_str.clone(),
    };

    // Delivery WAL: begin → applying → completed. The removal's ops are
    // recorded so a crash leaves a faithful APPLYING record; `recover_if_needed`
    // then rolls it back, and the removal journal (whose parts are still
    // registered) restores the files.
    let tx_id = wright_registry::delivery::begin_delivery(db, &command_str).await?;

    let refs = plan.part_refs(db).await?;
    let ops: Vec<(String, String, String, i64, Option<String>)> = refs
        .iter()
        .enumerate()
        .map(|(order, part)| {
            (
                part.name.clone(),
                part.hash.clone().unwrap_or_default(),
                "remove".to_string(),
                order as i64,
                part.hash.clone(),
            )
        })
        .collect();
    wright_registry::delivery::register_ops(db, tx_id, &ops).await?;
    wright_registry::delivery::begin_applying(db, tx_id).await?;

    let op_ids: std::collections::HashMap<String, i64> = db
        .get_ops_for_delivery(tx_id)
        .await?
        .into_iter()
        .map(|op| (op.part_name.clone(), op.id))
        .collect();

    let timing = crate::util::timing::WorkflowTiming::new();

    let mut batch = RemovalBatch::begin(db, root_dir, session, &refs).await?;

    // The batch-wide ignore set: dependents that are themselves being removed
    // in this run, so they do not block their dependency.
    let batch_targets: HashSet<String> = plan.targets.iter().cloned().collect();

    for name in &plan.targets {
        crate::cli_action!("Removing", "{}", name);
        let ignored: HashSet<String> = batch_targets
            .iter()
            .filter(|candidate| candidate.as_str() != name.as_str())
            .cloned()
            .collect();
        if let Some(op_id) = op_ids.get(name) {
            let _ = wright_registry::delivery::op_hooks_running(db, *op_id).await;
        }
        if let Err(e) = batch.remove_one(name, force, &ignored).await {
            let _ = batch.rollback().await;
            if let Some(op_id) = op_ids.get(name) {
                let _ = wright_registry::delivery::op_failed(db, *op_id, &e.to_string()).await;
            }
            let _ = wright_registry::delivery::rollback_delivery(db, tx_id).await;
            let _ = wright_registry::delivery::cleanup_delivery(db, tx_id).await;
            tracing::error!(event = "remove.failed", part_name = %name, error = %e, "Removal failed");
            return Err(WrightError::context(format!("remove {}", name), e));
        }
        if let Some(op_id) = op_ids.get(name) {
            let _ = wright_registry::delivery::op_done(db, *op_id).await;
        }
    }

    let removed = match batch.commit().await {
        Ok(removed) => removed,
        Err(e) => {
            let _ = wright_registry::delivery::rollback_delivery(db, tx_id).await;
            let _ = wright_registry::delivery::cleanup_delivery(db, tx_id).await;
            return Err(WrightError::context("failed to commit removal", e));
        }
    };

    wright_registry::delivery::complete_delivery(db, tx_id).await?;
    let _ = wright_registry::delivery::cleanup_delivery(db, tx_id).await;

    crate::cli_action!(
        "Finished",
        "remove in {}: {} part(s)",
        crate::util::timing::format_duration(timing.elapsed()),
        removed.len(),
    );

    Ok(())
}

struct RemovalPlan {
    /// User-supplied targets, echoed in the delivery command string.
    requested: Vec<String>,
    /// Expanded, ordered part names to remove.
    targets: Vec<String>,
}

impl RemovalPlan {
    async fn part_refs(&self, db: &InstalledDb) -> Result<Vec<PartRef>> {
        let mut refs = Vec::new();
        for name in &self.targets {
            let part = db
                .get_part(name)
                .await?
                .ok_or_else(|| WrightError::PartNotFound(name.clone()))?;
            refs.push(PartRef {
                name: name.clone(),
                hash: part.part_hash,
            });
        }
        Ok(refs)
    }
}

/// Expand and validate a removal request into an ordered, closed batch.
///
/// Every target is resolved through the universal plan/output identifier. With
/// `recursive`, each target's dependents join the batch; with `cascade`, so do
/// orphaned dependencies. The batch is then validated: a dependent outside the
/// batch is a blocker. Ordering guarantees dependents are removed before their
/// dependencies.
async fn plan_removal(
    db: &InstalledDb,
    parts: &[&str],
    recursive: bool,
    cascade: bool,
) -> Result<RemovalPlan> {
    let mut requested: Vec<String> = Vec::new();
    let mut members: HashSet<String> = HashSet::new();

    for target in parts {
        let ident = Identifier::parse(target)?;
        for part in crate::identify::resolve(db, &ident).await?.parts() {
            requested.push(part.name.clone());
            members.insert(part.name.clone());
        }
    }

    // Pull in dependents (recursive) and orphaned dependencies (cascade) until
    // the batch is closed under both relations.
    loop {
        let mut grew = false;

        if recursive {
            let current: Vec<String> = members.iter().cloned().collect();
            for name in current {
                for dependent in db.get_recursive_dependents(&name).await? {
                    if members.insert(dependent) {
                        grew = true;
                    }
                }
            }
        }
        if cascade {
            let current: Vec<String> = members.iter().cloned().collect();
            for name in current {
                for orphan in transaction::cascade_remove_list(db, &name).await? {
                    if members.insert(orphan) {
                        grew = true;
                    }
                }
            }
        }

        if !grew {
            break;
        }
    }

    // Validate: every dependent of a batch member must also be in the batch.
    let mut blockers: Vec<String> = Vec::new();
    for name in &members {
        for dependent in db.get_dependents(name).await? {
            if !members.contains(&dependent) {
                blockers.push(format!("{} (required by {})", name, dependent));
            }
        }
    }
    if !blockers.is_empty() {
        blockers.sort();
        blockers.dedup();
        return Err(WrightError::DependencyError(format!(
            "cannot remove: {}",
            blockers.join(", ")
        )));
    }

    // Order dependents before their dependencies.
    let members_vec: Vec<String> = members.into_iter().collect();
    let targets = transaction::order_removal_batch(db, &members_vec).await?;

    Ok(RemovalPlan { requested, targets })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wright_registry::database::{Dependency, NewPart, NewPlan};

    async fn test_db() -> InstalledDb {
        InstalledDb::open_in_memory().await.unwrap()
    }

    async fn add_plan(db: &InstalledDb, name: &str, outputs: &[&str]) {
        let plan_id = db
            .insert_plan(NewPlan {
                name,
                ..Default::default()
            })
            .await
            .unwrap();
        for output in outputs {
            db.insert_part(NewPart {
                name: output,
                plan_id,
                ..Default::default()
            })
            .await
            .unwrap();
        }
    }

    async fn link(db: &InstalledDb, from: &str, to: &str) {
        let part = db.get_part(from).await.unwrap().unwrap();
        db.insert_dependencies(
            part.id,
            &[Dependency {
                name: to.to_string(),
                version_constraint: None,
            }],
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn dry_run_expands_plan_and_output_targets() {
        let db = test_db().await;
        add_plan(&db, "llvm", &["clang", "lld"]).await;
        add_plan(&db, "zlib", &["zlib"]).await;

        execute_remove(
            &db,
            &["llvm", "zlib:*", "lld", "llvm:clang"],
            false,
            false,
            false,
            true,
            std::path::Path::new("/"),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn remove_rejects_ambiguous_bare_name() {
        let db = test_db().await;
        add_plan(&db, "gcc", &["gcc", "libstdc++"]).await;

        let err = execute_remove(
            &db,
            &["gcc"],
            false,
            false,
            false,
            true,
            std::path::Path::new("/"),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, WrightError::AmbiguousTarget(_)),
            "expected ambiguous target, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn remove_rejects_output_of_the_wrong_plan() {
        let db = test_db().await;
        add_plan(&db, "llvm", &["clang"]).await;
        add_plan(&db, "gcc", &["gcc"]).await;

        let err = execute_remove(
            &db,
            &["gcc:clang"],
            false,
            false,
            false,
            true,
            std::path::Path::new("/"),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, WrightError::ValidationError(_)),
            "expected validation error, got: {}",
            err
        );
    }

    /// A dependent outside the batch is a blocker; naming both makes it
    /// removable, and the plan orders the dependent first.
    #[tokio::test]
    async fn plan_blocks_outside_dependent_and_orders_batch() {
        let db = test_db().await;
        add_plan(&db, "lib", &["lib"]).await;
        add_plan(&db, "app", &["app"]).await;
        link(&db, "app", "lib").await;

        let blocked = plan_removal(&db, &["lib"], false, false).await;
        assert!(blocked.is_err(), "lib alone must be blocked by app");

        let plan = plan_removal(&db, &["lib", "app"], false, false)
            .await
            .unwrap();
        let app_at = plan.targets.iter().position(|n| n == "app").unwrap();
        let lib_at = plan.targets.iter().position(|n| n == "lib").unwrap();
        assert!(app_at < lib_at, "dependent app must precede lib");
    }

    /// `--recursive` closes the batch over dependents, so a bare target no
    /// longer needs the dependent named.
    #[tokio::test]
    async fn recursive_closes_over_dependents() {
        let db = test_db().await;
        add_plan(&db, "lib", &["lib"]).await;
        add_plan(&db, "app", &["app"]).await;
        link(&db, "app", "lib").await;

        let plan = plan_removal(&db, &["lib"], true, false).await.unwrap();
        assert!(plan.targets.contains(&"app".to_string()));
        assert!(plan.targets.contains(&"lib".to_string()));
    }
}
