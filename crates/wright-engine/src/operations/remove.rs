use std::collections::HashSet;

use crate::error::{Result, WrightError};
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
    let mut parts_owned: Vec<String> = Vec::new();
    for target in parts {
        if let Some((_plan, output)) = target.split_once(':') {
            parts_owned.push(output.trim().to_string());
        } else {
            let plan_parts = db.get_parts_by_plan(target).await.unwrap_or_default();
            if !plan_parts.is_empty() {
                for p in plan_parts {
                    parts_owned.push(p.name);
                }
            } else {
                parts_owned.push(target.to_string());
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
            .map_err(|e| WrightError::RemoveError(format!("failed to plan removal order: {}", e)))?
    };

    if dry_run {
        // Read-only preview: expand the same dependents/cascade lists the
        // removal loop would walk, in the same order, without starting a
        // delivery transaction.
        let mut planned: Vec<String> = Vec::new();
        for name in &removal_order {
            if recursive {
                let dependents = db.get_recursive_dependents(name).await.map_err(|e| {
                    WrightError::DatabaseError(format!(
                        "failed to resolve dependents of {}: {}",
                        name, e
                    ))
                })?;
                planned.extend(dependents);
            }
            planned.push(name.clone());
            if cascade {
                let orphans = transaction::cascade_remove_list(db, name)
                    .await
                    .map_err(|e| {
                        WrightError::RemoveError(format!(
                            "failed to compute cascade list for {}: {}",
                            name, e
                        ))
                    })?;
                planned.extend(orphans);
            }
        }
        println!("[dry-run] remove -> {}", root_dir.display());
        println!("[dry-run] would remove {} part(s):", planned.len());
        for name in &planned {
            println!("  {}", name);
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

    let workflow_t0 = std::time::Instant::now();
    let mut total_removed = 0usize;

    for name in &removal_order {
        if recursive {
            let dependents = db.get_recursive_dependents(name).await.map_err(|e| {
                WrightError::DatabaseError(format!(
                    "failed to resolve dependents of {}: {}",
                    name, e
                ))
            })?;

            if !dependents.is_empty() {
                crate::cli_action!(
                    "Cascading",
                    "{} depends on {}: {}",
                    dependents.len(),
                    name,
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
                    return Err(WrightError::RemoveError(format!("remove {}: {}", dep, e)));
                }
                total_removed += 1;
            }
        }

        let cascade_list = if cascade {
            let list = transaction::cascade_remove_list(db, name)
                .await
                .map_err(|e| {
                    WrightError::RemoveError(format!(
                        "failed to compute cascade list for {}: {}",
                        name, e
                    ))
                })?;
            if !list.is_empty() {
                crate::cli_action!("Cascading", "orphans of {}: {}", name, list.join(", "));
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
            return Err(WrightError::RemoveError(format!("remove {}: {}", name, e)));
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
                return Err(WrightError::RemoveError(format!(
                    "remove {}: {}",
                    orphan, e
                )));
            }
            total_removed += 1;
        }
    }

    wright_state::delivery::complete_delivery(db, tx_id).await?;
    let _ = wright_state::delivery::cleanup_delivery(db, tx_id).await;

    let elapsed = workflow_t0.elapsed().as_secs_f64();
    crate::cli_action!(
        "Finished",
        "remove in {}: {} part(s)",
        crate::foundry::logging::format_duration(elapsed),
        total_removed,
    );

    Ok(())
}
