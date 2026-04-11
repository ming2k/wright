use std::collections::HashSet;
use std::path::Path;

use tracing::{debug, info, warn};

use crate::database::{Database, FileType, InstalledPart};
use crate::error::{Result, WrightError};

use super::get_hook;
use crate::transaction::hooks::run_install_script;

pub fn remove_part(db: &Database, name: &str, root_dir: &Path, force: bool) -> Result<()> {
    remove_part_with_ignored_dependents(db, name, root_dir, force, &HashSet::new())
}

pub fn remove_part_with_ignored_dependents(
    db: &Database,
    name: &str,
    root_dir: &Path,
    force: bool,
    ignored_dependents: &HashSet<String>,
) -> Result<()> {
    let pkg = db
        .get_part(name)?
        .ok_or_else(|| WrightError::PartNotFound(name.to_string()))?;

    let mut dependents = collect_removal_dependents(db, &pkg, name, ignored_dependents)?;

    if !ignored_dependents.is_empty() {
        dependents.retain(|(dep_name, _)| !ignored_dependents.contains(dep_name));
    }

    if !dependents.is_empty() {
        let mut link_dependents = Vec::new();
        let mut other_dependents = Vec::new();

        for (dep_name, dep_type) in &dependents {
            if dep_type == "link" {
                link_dependents.push(dep_name.clone());
            } else {
                other_dependents.push(dep_name.clone());
            }
        }

        let all_deps_names: Vec<String> = dependents.iter().map(|(n, _)| n.clone()).collect();
        let deps_str = all_deps_names.join(", ");

        if force {
            warn!(
                "Warning: forcing removal of {} which is depended on by: {}",
                name, deps_str
            );
        } else {
            if !link_dependents.is_empty() {
                return Err(WrightError::DependencyError(format!(
                    "CRITICAL: Cannot remove '{}' because it is a LINK dependency of: {}. \
                     Removing it will cause these parts to CRASH. Use --force to override.",
                    name,
                    link_dependents.join(", ")
                )));
            }
            return Err(WrightError::DependencyError(format!(
                "cannot remove '{}': required by {}",
                name, deps_str
            )));
        }
    }

    if let Some(ref content) = pkg.install_scripts {
        if let Some(script) = get_hook(content, "pre_remove") {
            debug!("Running pre_remove hook for {}", name);
            if let Err(e) = run_install_script(&script, root_dir) {
                warn!("pre_remove script failed (continuing removal): {}", e);
            }
        }
    }

    let tx_id = db.record_transaction("remove", name, Some(&pkg.version), None, "pending", None)?;
    let files = db.get_files(pkg.id)?;

    let file_paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let other_owners_map = db.get_other_owners_batch(pkg.id, &file_paths)?;

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
                if full_path.exists() || full_path.symlink_metadata().is_ok() {
                    std::fs::remove_file(&full_path).map_err(|e| {
                        WrightError::RemoveError(format!(
                            "failed to remove {}: {}",
                            full_path.display(),
                            e
                        ))
                    })?;
                }
            }
            FileType::Directory => {
                if full_path.is_dir() {
                    let _ = std::fs::remove_dir(&full_path);
                }
            }
        }
    }

    if let Some(ref content) = pkg.install_scripts {
        if let Some(script) = get_hook(content, "post_remove") {
            debug!("Running post_remove hook for {}", name);
            if let Err(e) = run_install_script(&script, root_dir) {
                warn!("post_remove script failed (continuing removal): {}", e);
            }
        }
    }

    db.remove_part(name)?;
    db.update_transaction_status(tx_id, "completed")?;

    info!("Removed {}", name);
    Ok(())
}

fn collect_removal_dependents(
    db: &Database,
    pkg: &InstalledPart,
    name: &str,
    ignored_dependents: &HashSet<String>,
) -> Result<Vec<(String, String)>> {
    let mut dependents = db.get_dependents(name)?;

    let provides_list = db.get_provides(pkg.id)?;
    for virtual_name in &provides_list {
        let virtual_dependents = db.get_dependents(virtual_name)?;
        for (dep_name, dep_type) in virtual_dependents {
            let remaining_providers: Vec<String> = db
                .find_providers(virtual_name)?
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

pub fn order_removal_batch(db: &Database, targets: &[String]) -> Result<Vec<String>> {
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
        )?;
    }

    Ok(ordered)
}

fn visit_removal_target(
    db: &Database,
    name: &str,
    target_set: &HashSet<String>,
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
    ordered: &mut Vec<String>,
) -> Result<()> {
    if visited.contains(name) {
        return Ok(());
    }
    if !visiting.insert(name.to_string()) {
        return Ok(());
    }

    let pkg = db
        .get_part(name)?
        .ok_or_else(|| WrightError::PartNotFound(name.to_string()))?;
    let batch_ignored: HashSet<String> = target_set
        .iter()
        .filter(|candidate| candidate.as_str() != name)
        .cloned()
        .collect();
    let dependents = collect_removal_dependents(db, &pkg, name, &batch_ignored)?;
    let mut next: Vec<String> = dependents
        .into_iter()
        .map(|(dep_name, _)| dep_name)
        .filter(|dep_name| target_set.contains(dep_name))
        .collect();
    next.sort();
    next.dedup();

    for dep_name in next {
        visit_removal_target(db, &dep_name, target_set, visiting, visited, ordered)?;
    }

    visiting.remove(name);
    visited.insert(name.to_string());
    ordered.push(name.to_string());
    Ok(())
}

pub fn cascade_remove_list(db: &Database, name: &str) -> Result<Vec<String>> {
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    visited.insert(name.to_string());
    cascade_collect(db, name, &mut visited, &mut result)?;
    Ok(result)
}

fn cascade_collect(
    db: &Database,
    name: &str,
    visited: &mut HashSet<String>,
    result: &mut Vec<String>,
) -> Result<()> {
    let orphans = db.get_orphan_dependencies(name)?;
    for orphan in orphans {
        if visited.contains(&orphan) {
            continue;
        }
        visited.insert(orphan.clone());
        cascade_collect(db, &orphan, visited, result)?;
        result.push(orphan);
    }
    Ok(())
}
