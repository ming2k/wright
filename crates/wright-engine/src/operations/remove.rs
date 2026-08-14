use std::collections::HashSet;

use crate::error::{Result, WrightError};
use crate::identify::Identifier;
use crate::transaction;
use wright_state::database::{InstalledDb, SessionContext};

pub async fn execute_remove(
    db: &InstalledDb,
    parts: &[&str],
    force: bool,
    recursive: bool,
    cascade: bool,
    dry_run: bool,
    root_dir: &std::path::Path,
) -> Result<()> {
    // Expand every target through the universal plan/output identifier:
    // plan-level targets resolve to all deployed outputs of the plan,
    // output-level targets to a single part. Duplicates collapse.
    let mut parts_owned: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for target in parts {
        let ident = Identifier::parse(target)?;
        let resolved = crate::identify::resolve(db, &ident).await?;
        for part in resolved.parts() {
            if seen.insert(part.name.clone()) {
                parts_owned.push(part.name.clone());
            }
        }
    }

    let batch_targets: HashSet<String> = if recursive {
        HashSet::new()
    } else {
        parts_owned.iter().cloned().collect()
    };

    let removal_order = if recursive {
        parts_owned.clone()
    } else {
        transaction::order_removal_batch(db, &parts_owned)
            .await
            .map_err(|e| WrightError::context("failed to plan removal order", e))?
    };

    if dry_run {
        // Read-only preview: expand the same dependents/cascade lists the
        // removal loop would walk, in the same order, without starting a
        // delivery transaction.
        let mut planned: Vec<String> = Vec::new();
        for name in &removal_order {
            if recursive {
                let dependents = db.get_recursive_dependents(name).await.map_err(|e| {
                    WrightError::context(format!("failed to resolve dependents of {}", name), e)
                })?;
                planned.extend(dependents);
            }
            planned.push(name.clone());
            if cascade {
                let orphans = transaction::cascade_remove_list(db, name)
                    .await
                    .map_err(|e| {
                        WrightError::context(
                            format!("failed to compute cascade list for {}", name),
                            e,
                        )
                    })?;
                planned.extend(orphans);
            }
        }
        crate::outln!("[dry-run] remove -> {}", root_dir.display());
        crate::outln!("[dry-run] would remove {} part(s):", planned.len());
        for name in &planned {
            crate::outln!("  {}", name);
        }
        return Ok(());
    }

    let command_str = format!("remove {}", parts_owned.join(" "));
    let tx_id = wright_state::delivery::begin_delivery(db, &command_str).await?;
    let session = SessionContext {
        id: format!(
            "{:x}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ),
        command: command_str,
    };

    let timing = crate::util::timing::WorkflowTiming::new();
    let mut total_removed = 0usize;

    for name in &removal_order {
        if recursive {
            let dependents = db.get_recursive_dependents(name).await.map_err(|e| {
                WrightError::context(format!("failed to resolve dependents of {}", name), e)
            })?;

            if !dependents.is_empty() {
                crate::cli_action!(
                    "Cascading",
                    "{} dependents of {}:\n{}{}",
                    dependents.len(),
                    name,
                    crate::util::logging::continuation_indent(),
                    dependents.join(", ")
                );
            }

            for dep in &dependents {
                crate::cli_action!("Removing", "{}", dep);
                if let Err(e) =
                    transaction::remove_part(db, dep, root_dir, true, session.clone()).await
                {
                    let _ = wright_state::delivery::rollback_delivery(db, tx_id).await;
                    let _ = wright_state::delivery::cleanup_delivery(db, tx_id).await;
                    tracing::error!(event = "remove.failed", part_name = %dep, error = %e, "Removal failed");
                    return Err(WrightError::context(format!("remove {}", dep), e));
                }
                total_removed += 1;
            }
        }

        let cascade_list = if cascade {
            let list = transaction::cascade_remove_list(db, name)
                .await
                .map_err(|e| {
                    WrightError::context(format!("failed to compute cascade list for {}", name), e)
                })?;
            if !list.is_empty() {
                crate::cli_action!(
                    "Cascading",
                    "{} orphans of {}:\n{}{}",
                    list.len(),
                    name,
                    crate::util::logging::continuation_indent(),
                    list.join(", ")
                );
            }
            list
        } else {
            Vec::new()
        };

        crate::cli_action!("Removing", "{}", name);
        let result = if recursive {
            transaction::remove_part(db, name, root_dir, force || recursive, session.clone()).await
        } else {
            let ignored_dependents: HashSet<String> = batch_targets
                .iter()
                .filter(|candidate| candidate.as_str() != *name)
                .cloned()
                .collect();
            transaction::remove_part_with_ignored_dependents(
                db,
                name,
                root_dir,
                force,
                &ignored_dependents,
                session.clone(),
            )
            .await
        };

        if let Err(e) = result {
            let _ = wright_state::delivery::rollback_delivery(db, tx_id).await;
            let _ = wright_state::delivery::cleanup_delivery(db, tx_id).await;
            tracing::error!(event = "remove.failed", part_name = %name, error = %e, "Removal failed");
            return Err(WrightError::context(format!("remove {}", name), e));
        }
        total_removed += 1;

        for orphan in &cascade_list {
            crate::cli_action!("Removing", "{}", orphan);
            if let Err(e) =
                transaction::remove_part(db, orphan, root_dir, true, session.clone()).await
            {
                let _ = wright_state::delivery::rollback_delivery(db, tx_id).await;
                let _ = wright_state::delivery::cleanup_delivery(db, tx_id).await;
                tracing::error!(event = "remove.failed", part_name = %orphan, error = %e, "Removal failed");
                return Err(WrightError::context(format!("remove {}", orphan), e));
            }
            total_removed += 1;
        }
    }

    wright_state::delivery::complete_delivery(db, tx_id).await?;
    let _ = wright_state::delivery::cleanup_delivery(db, tx_id).await;

    crate::cli_action!(
        "Finished",
        "remove in {}: {} part(s)",
        crate::util::timing::format_duration(timing.elapsed()),
        total_removed,
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wright_state::database::{NewPart, NewPlan};

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

    #[tokio::test]
    async fn dry_run_expands_plan_and_output_targets() {
        let db = test_db().await;
        add_plan(&db, "llvm", &["clang", "lld"]).await;
        add_plan(&db, "zlib", &["zlib"]).await;

        // Plan-level, absolute output-level, and wildcard forms all resolve;
        // dry-run stays read-only.
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
}
