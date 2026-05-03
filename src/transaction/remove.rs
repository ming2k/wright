use std::collections::HashSet;
use std::path::Path;

use tracing::{info, warn};

use crate::database::{FileType, InstalledDb, InstalledPart};
use crate::error::{Result, WrightError};

use super::get_hook;
use crate::transaction::hooks::{log_running_hook, run_install_script};

use futures_util::future::BoxFuture;
use futures_util::FutureExt;

pub async fn remove_part(db: &InstalledDb, name: &str, root_dir: &Path, force: bool) -> Result<()> {
    remove_part_with_ignored_dependents(db, name, root_dir, force, &HashSet::new()).await
}

pub async fn remove_part_with_ignored_dependents(
    db: &InstalledDb,
    name: &str,
    root_dir: &Path,
    force: bool,
    ignored_dependents: &HashSet<String>,
) -> Result<()> {
    let part = db
        .get_part(name)
        .await?
        .ok_or_else(|| WrightError::PartNotFound(name.to_string()))?;

    let mut dependents = collect_removal_dependents(db, &part, name, ignored_dependents).await?;

    if !ignored_dependents.is_empty() {
        dependents.retain(|(dep_name, _)| !ignored_dependents.contains(dep_name));
    }

    if !dependents.is_empty() {
        let deps_str: String = dependents.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>().join(", ");

        if force {
            warn!(
                "Warning: forcing removal of {} which is depended on by: {}",
                name, deps_str
            );
        } else {
            return Err(WrightError::DependencyError(format!(
                "cannot remove '{}': required by {}",
                name, deps_str
            )));
        }
    }

    if let Some(ref content) = part.install_scripts {
        if let Some(script) = get_hook(content, "pre_remove") {
            log_running_hook(name, "pre_remove");
            if let Err(e) = run_install_script(&script, root_dir).await {
                warn!("pre_remove script failed (continuing removal): {}", e);
            }
        }
    }

    let tx_id = db
        .record_transaction("remove", name, Some(&part.version), None, "pending", None)
        .await?;
    let files = db.get_files(part.id).await?;

    let file_paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let other_owners_map = db.get_other_owners_batch(part.id, &file_paths).await?;

    for file in files.iter().rev() {
        let full_path = root_dir.join(file.path.trim_start_matches('/'));
        if file.is_config {
            info!("Preserving config file: {}", file.path);
            continue;
        }

        let other_owners = other_owners_map
            .get(&file.path)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        if !other_owners.is_empty() {
            tracing::debug!(
                "Path {} is also owned by: {}. Skipping deletion.",
                file.path,
                other_owners.join(", ")
            );
            continue;
        }

        match file.file_type {
            FileType::File | FileType::Symlink => {
                let metadata = tokio::fs::symlink_metadata(&full_path).await;
                if metadata.is_ok() {
                    tokio::fs::remove_file(&full_path).await.map_err(|e| {
                        WrightError::RemoveError(format!(
                            "failed to remove {}: {}",
                            full_path.display(),
                            e
                        ))
                    })?;
                }
            }
            FileType::Directory => {
                let metadata = tokio::fs::metadata(&full_path).await;
                if metadata.map(|m| m.is_dir()).unwrap_or(false) {
                    let _ = tokio::fs::remove_dir(&full_path).await;
                }
            }
        }
    }

    if let Some(ref content) = part.install_scripts {
        if let Some(script) = get_hook(content, "post_remove") {
            log_running_hook(name, "post_remove");
            if let Err(e) = run_install_script(&script, root_dir).await {
                warn!("post_remove script failed (continuing removal): {}", e);
            }
        }
    }

    let diversions = db.get_all_diverted_files(part.id).await.unwrap_or_default();

    db.remove_part(name).await?;

    // Restore diverted files
    for (original_path, diverted_path) in diversions {
        let full_original = root_dir.join(original_path.trim_start_matches('/'));
        let full_diverted = root_dir.join(diverted_path.trim_start_matches('/'));

        let metadata = tokio::fs::symlink_metadata(&full_diverted).await;
        if metadata.is_ok() {
            info!("Restoring diverted file: {}", original_path);
            if let Err(e) = tokio::fs::rename(&full_diverted, &full_original).await {
                warn!("Failed to restore diverted file {}: {}", original_path, e);
            }
        }
    }
    let _ = db.remove_shadowed_records(part.id).await;

    db.update_transaction_status(tx_id, "completed").await?;

    info!("Removed {}", name);
    Ok(())
}

async fn collect_removal_dependents(
    db: &InstalledDb,
    part: &InstalledPart,
    name: &str,
    ignored_dependents: &HashSet<String>,
) -> Result<Vec<(String, String)>> {
    let mut dependents = db.get_dependents(name).await?;

    let provides_list = db.get_provides(part.id).await?;
    for virtual_name in &provides_list {
        let virtual_dependents = db.get_dependents(virtual_name).await?;
        for (dep_name, dep_type) in virtual_dependents {
            let remaining_providers: Vec<String> = db
                .find_providers(virtual_name)
                .await?
                .into_iter()
                .filter(|p| p != name && !ignored_dependents.contains(p))
                .collect();
            if remaining_providers.is_empty()
                && !dependents
                    .iter()
                    .any(|(existing_name, _)| existing_name == &dep_name)
            {
                dependents.push((dep_name, dep_type));
            }
        }
    }

    Ok(dependents)
}

pub async fn order_removal_batch(db: &InstalledDb, targets: &[String]) -> Result<Vec<String>> {
    let target_set: HashSet<String> = targets.iter().cloned().collect();
    let mut ordered = Vec::new();
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();

    for name in targets {
        visit_removal_target(
            db,
            name,
            &target_set,
            &mut visiting,
            &mut visited,
            &mut ordered,
        )
        .await?;
    }

    Ok(ordered)
}

fn visit_removal_target<'a>(
    db: &'a InstalledDb,
    name: &'a str,
    target_set: &'a HashSet<String>,
    visiting: &'a mut HashSet<String>,
    visited: &'a mut HashSet<String>,
    ordered: &'a mut Vec<String>,
) -> BoxFuture<'a, Result<()>> {
    async move {
        if visited.contains(name) {
            return Ok(());
        }
        if !visiting.insert(name.to_string()) {
            return Ok(());
        }

        let part = db
            .get_part(name)
            .await?
            .ok_or_else(|| WrightError::PartNotFound(name.to_string()))?;
        let batch_ignored: HashSet<String> = target_set
            .iter()
            .filter(|candidate| candidate.as_str() != name)
            .cloned()
            .collect();
        let dependents = collect_removal_dependents(db, &part, name, &batch_ignored).await?;
        let mut next: Vec<String> = dependents
            .into_iter()
            .map(|(dep_name, _)| dep_name)
            .filter(|dep_name| target_set.contains(dep_name))
            .collect();
        next.sort();
        next.dedup();

        for dep_name in next {
            visit_removal_target(db, &dep_name, target_set, visiting, visited, ordered).await?;
        }

        visiting.remove(name);
        visited.insert(name.to_string());
        ordered.push(name.to_string());
        Ok(())
    }
    .boxed()
}

pub async fn cascade_remove_list(db: &InstalledDb, name: &str) -> Result<Vec<String>> {
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    visited.insert(name.to_string());
    cascade_collect(db, name, &mut visited, &mut result).await?;
    Ok(result)
}

fn cascade_collect<'a>(
    db: &'a InstalledDb,
    name: &'a str,
    visited: &'a mut HashSet<String>,
    result: &'a mut Vec<String>,
) -> BoxFuture<'a, Result<()>> {
    async move {
        let orphans = db.get_orphan_dependencies(name).await?;
        for orphan in orphans {
            if visited.contains(&orphan) {
                continue;
            }
            visited.insert(orphan.clone());
            cascade_collect(db, &orphan, visited, result).await?;
            result.push(orphan);
        }
        Ok(())
    }
    .boxed()
}
