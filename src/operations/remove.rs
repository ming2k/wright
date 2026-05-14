use std::collections::HashSet;

use crate::database::{InstalledDb, SessionContext};
use crate::error::{Result, WrightError};
use crate::transaction;

pub async fn execute_remove(
    db: &InstalledDb,
    parts: &[&str],
    force: bool,
    recursive: bool,
    cascade: bool,
    root_dir: &std::path::Path,
) -> Result<()> {
    let parts_owned: Vec<String> = parts.iter().map(|s| s.to_string()).collect();

    let command_str = format!("remove {}", parts_owned.join(" "));
    let tx_id = crate::delivery::begin_delivery(db, &command_str).await?;
    let session = SessionContext {
        id: format!(
            "{:x}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ),
        command: command_str,
    };

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
                    let _ = crate::delivery::rollback_delivery(db, tx_id).await;
                    let _ = crate::delivery::cleanup_delivery(db, tx_id).await;
                    tracing::error!(event = "remove.failed", part_name = %dep, error = %e, "Removal failed");
                    std::process::exit(1);
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
                crate::cli_action!(
                    "Cascading",
                    "orphans of {}: {}",
                    name,
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
            let _ = crate::delivery::rollback_delivery(db, tx_id).await;
            let _ = crate::delivery::cleanup_delivery(db, tx_id).await;
            tracing::error!(event = "remove.failed", part_name = %name, error = %e, "Removal failed");
            std::process::exit(1);
        }
        total_removed += 1;

        for orphan in &cascade_list {
            crate::cli_action!("Removing", "{}", orphan);
            if let Err(e) =
                transaction::remove_part(db, orphan, root_dir, true, session.clone()).await
            {
                let _ = crate::delivery::rollback_delivery(db, tx_id).await;
                let _ = crate::delivery::cleanup_delivery(db, tx_id).await;
                tracing::error!(event = "remove.failed", part_name = %orphan, error = %e, "Removal failed");
                std::process::exit(1);
            }
            total_removed += 1;
        }
    }

    crate::delivery::complete_delivery(db, tx_id).await?;
    let _ = crate::delivery::cleanup_delivery(db, tx_id).await;

    let elapsed = workflow_t0.elapsed().as_secs_f64();
    crate::cli_action!(
        "Finished",
        "remove in {}: {} part(s)",
        crate::foundry::logging::format_duration(elapsed),
        total_removed,
    );

    Ok(())
}
