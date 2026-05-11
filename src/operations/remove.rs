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
    for name in parts {
        if let Some(part) = db.get_part(name).await? {
            if part.origin == crate::database::Origin::External {
                tracing::error!(
                    "'{}' is externally provided. Use 'wright unassume {}' instead of 'remove'.",
                    name,
                    name
                );
                std::process::exit(1);
            }
        }
    }

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

    for name in &removal_order {
        if recursive {
            let dependents = db.get_recursive_dependents(name).await.map_err(|e| {
                WrightError::DatabaseError(format!(
                    "failed to resolve dependents of {}: {}",
                    name, e
                ))
            })?;

            if !dependents.is_empty() {
                println!(
                    "will also remove (depends on {}): {}",
                    name,
                    dependents.join(", ")
                );
            }

            for dep in &dependents {
                match transaction::remove_part(db, dep, root_dir, true, session.clone()).await {
                    Ok(()) => println!("removed: {}", dep),
                    Err(e) => {
                        let _ = crate::delivery::rollback_delivery(db, tx_id).await;
                        let _ = crate::delivery::cleanup_delivery(db, tx_id).await;
                        tracing::error!("removing {}: {}", dep, e);
                        std::process::exit(1);
                    }
                }
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
                println!(
                    "will also remove orphan dependencies of {}: {}",
                    name,
                    list.join(", ")
                );
            }
            list
        } else {
            Vec::new()
        };

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

        match result {
            Ok(()) => println!("removed: {}", name),
            Err(e) => {
                let _ = crate::delivery::rollback_delivery(db, tx_id).await;
                let _ = crate::delivery::cleanup_delivery(db, tx_id).await;
                tracing::error!("removing {}: {}", name, e);
                std::process::exit(1);
            }
        }

        for orphan in &cascade_list {
            match transaction::remove_part(db, orphan, root_dir, true, session.clone()).await {
                Ok(()) => println!("removed: {}", orphan),
                Err(e) => {
                    let _ = crate::delivery::rollback_delivery(db, tx_id).await;
                    let _ = crate::delivery::cleanup_delivery(db, tx_id).await;
                    tracing::error!("removing {}: {}", orphan, e);
                    std::process::exit(1);
                }
            }
        }
    }

    crate::delivery::complete_delivery(db, tx_id).await?;
    let _ = crate::delivery::cleanup_delivery(db, tx_id).await;

    Ok(())
}
