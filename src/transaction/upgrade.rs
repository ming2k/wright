use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{debug, info, warn};

use crate::database::{
    Dependency, FileType, InstalledDb, NewPart, TransactionOperation,
};
use crate::error::{Result, WrightError};
use crate::part::archive;
use crate::part::version::{self, Version};
use crate::transaction::context::TransactionContext;
use crate::transaction::fs::{collect_config_paths, collect_file_entries, copy_entries_to_root};
use crate::transaction::hooks::{log_running_hook, read_hooks, run_install_script};

use super::{log_debug_timing, self_replace_provides_conflicts};

pub async fn upgrade_part(
    db: &InstalledDb,
    part_path: &Path,
    root_dir: &Path,
    force: bool,
    run_hooks: bool,
) -> Result<()> {
    let overall_start = Instant::now();

    let staging_dir = root_dir.join("var/lib/wright/staging");
    let _ = tokio::fs::create_dir_all(&staging_dir).await;
    let temp_dir = tempfile::tempdir_in(&staging_dir)
        .or_else(|_| tempfile::tempdir())
        .map_err(|e| WrightError::UpgradeError(format!("failed to create temp dir: {}", e)))?;
    let mut phase_start = Instant::now();

    // Blocking call to extract_part, but it's mainly I/O.
    // We could wrap it in spawn_blocking if it's too slow.
    let (partinfo, part_hash) = archive::extract_part(part_path, temp_dir.path())?;

    log_debug_timing(
        "upgrade",
        &partinfo.name,
        "archive extraction",
        phase_start.elapsed(),
    );

    let installed_part = db.get_part(&partinfo.name).await?.ok_or_else(|| {
        WrightError::UpgradeError(format!(
            "part '{}' is not installed, use install instead",
            partinfo.name
        ))
    })?;

    let installed_plan = db
        .get_plan_by_id(installed_part.plan_id)
        .await?
        .ok_or_else(|| {
            WrightError::UpgradeError(format!(
                "plan for part '{}' not found in database",
                partinfo.name
            ))
        })?;

    let installed_epoch = installed_plan.epoch as u32;
    let new_epoch = partinfo.plan.epoch;
    if !force {
        let is_newer = if new_epoch != installed_epoch {
            new_epoch > installed_epoch
        } else {
            match (
                Version::parse(&installed_plan.version).ok(),
                Version::parse(&partinfo.plan.version).ok(),
            ) {
                (Some(installed_v), Some(new_v)) => {
                    if new_v != installed_v {
                        new_v > installed_v
                    } else {
                        partinfo.plan.release > installed_plan.release as u32
                    }
                }
                _ => {
                    // Fallback to string comparison when versions can't be parsed
                    // (e.g., empty versions)
                    let ord = partinfo.plan.version.cmp(&installed_plan.version);
                    if ord != std::cmp::Ordering::Equal {
                        ord == std::cmp::Ordering::Greater
                    } else {
                        partinfo.plan.release > installed_plan.release as u32
                    }
                }
            }
        };
        if !is_newer {
            let installed_ver_rel = if installed_plan.version.is_empty() {
                format!("{}", installed_plan.release)
            } else {
                format!("{}-{}", installed_plan.version, installed_plan.release)
            };
            let new_ver_rel = if partinfo.plan.version.is_empty() {
                format!("{}", partinfo.plan.release)
            } else {
                format!("{}-{}", partinfo.plan.version, partinfo.plan.release)
            };
            return Err(WrightError::UpgradeError(format!(
                "{} {} is not newer than installed {}",
                partinfo.name, new_ver_rel, installed_ver_rel,
            )));
        }
    }

    let (hooks_content, hooks) = read_hooks(temp_dir.path());
    phase_start = Instant::now();
    let new_entries = collect_file_entries(temp_dir.path(), &partinfo)?;
    log_debug_timing(
        "upgrade",
        &partinfo.name,
        "file scan and metadata collection",
        phase_start.elapsed(),
    );

    info!("Upgrading {}: {} files", partinfo.name, new_entries.len());

    phase_start = Instant::now();
    let file_paths: Vec<&str> = new_entries
        .iter()
        .filter(|e| e.file_type == FileType::File)
        .map(|e| e.path.as_str())
        .collect();
    let owners = db.find_owners_batch(&file_paths).await?;
    let mut shadows = Vec::new();
    let mut divert_paths = HashSet::new();
    for entry in &new_entries {
        if entry.file_type == FileType::File {
            if let Some(owner) = owners.get(&entry.path) {
                if *owner != partinfo.name {
                    warn!(
                        "[{}] diverted {} (owned by {})",
                        partinfo.name,
                        crate::util::compact_path(&entry.path),
                        owner
                    );
                    shadows.push((entry.path.clone(), owner.clone()));
                    divert_paths.insert(entry.path.clone());
                }
            }
        }
    }
    log_debug_timing(
        "upgrade",
        &partinfo.name,
        "owner conflict check",
        phase_start.elapsed(),
    );

    let mut tx = TransactionContext::begin(
        db,
        TransactionOperation::Upgrade,
        &partinfo.name,
        Some(&installed_plan.version),
        Some(&partinfo.plan.version),
    )
    .await?;

    let existing_files = db.get_files(installed_part.id).await?;
    let new_paths: HashSet<&str> = new_entries.iter().map(|e| e.path.as_str()).collect();

    let backup_dir = tempfile::tempdir()
        .map_err(|e| WrightError::UpgradeError(format!("failed to create backup dir: {}", e)))?;

    // Perform backup. For now, doing it sequentially but with async calls for safety.
    // Parallelizing async I/O can be done with join_all or similar.
    let overlapping: Vec<_> = existing_files
        .iter()
        .filter(|f| new_paths.contains(f.path.as_str()))
        .collect();

    for file in overlapping {
        let full_path = root_dir.join(file.path.trim_start_matches('/'));
        if !full_path.exists() && full_path.symlink_metadata().is_err() {
            continue;
        }

        if file.file_type == FileType::File {
            let backup_path = backup_dir.path().join(file.path.trim_start_matches('/'));
            if let Some(parent) = backup_path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    WrightError::UpgradeError(format!(
                        "failed to create backup directory {}: {}",
                        parent.display(),
                        e
                    ))
                })?;
            }
            // Prefer hard_link (instant, no data copy) with copy fallback.
            let result = match tokio::fs::hard_link(&full_path, &backup_path).await {
                Ok(()) => Ok(()),
                Err(_) => tokio::fs::copy(&full_path, &backup_path).await.map(|_| ()),
            };

            match result {
                Ok(()) => {
                    tx.rollback_state().record_backup(full_path, backup_path);
                }
                Err(e) => {
                    return Err(WrightError::UpgradeError(format!(
                        "failed to backup {}: {}",
                        full_path.display(),
                        e
                    )));
                }
            }
        } else if file.file_type == FileType::Symlink {
            if let Ok(target) = tokio::fs::read_link(&full_path).await {
                tx.rollback_state()
                    .record_symlink_backup(full_path, target.to_string_lossy().to_string());
            }
        }
    }

    if run_hooks {
        if let Some(ref script) = hooks.pre_install {
            log_running_hook(&partinfo.name, "pre_install");
            phase_start = Instant::now();
            if let Err(e) =
                run_install_script(script, root_dir, &partinfo.name, "pre_install").await
            {
                warn!("pre_install script failed: {}", e);
            }
            log_debug_timing(
                "upgrade",
                &partinfo.name,
                "pre_install hook",
                phase_start.elapsed(),
            );
        }
    }

    let config_paths = collect_config_paths(&new_entries);
    phase_start = Instant::now();

    // copy_entries_to_root should probably also be async.
    // For now I'll assume it's still sync and wraps internal tokio calls if needed,
    // but better to refactor it to async too.
    let preserved_configs = match copy_entries_to_root(
        &new_entries,
        temp_dir.path(),
        root_dir,
        tx.rollback_state(),
        None,
        &config_paths,
        &divert_paths,
    )
    .await
    {
        Ok(paths) => paths,
        Err(e) => {
            warn!("Upgrade failed for {}, rolling back: {}", partinfo.name, e);
            tx.rollback().await?;
            return Err(e);
        }
    };

    log_debug_timing(
        "upgrade",
        &partinfo.name,
        "filesystem copy into target root",
        phase_start.elapsed(),
    );
    for path in preserved_configs {
        info!("Preserved config for {}: {}", partinfo.name, path);
    }

    let to_delete_paths: Vec<&str> = existing_files
        .iter()
        .filter(|f| !new_paths.contains(f.path.as_str()) && !f.is_config)
        .map(|f| f.path.as_str())
        .collect();
    let other_owners_map = db
        .get_other_owners_batch(installed_part.id, &to_delete_paths)
        .await?;

    for file in existing_files.iter().rev() {
        if new_paths.contains(file.path.as_str()) {
            continue;
        }

        if file.is_config {
            info!(
                "Preserving config file for {}: {}",
                partinfo.name, file.path
            );
            continue;
        }

        let other_owners = other_owners_map
            .get(&file.path)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        if !other_owners.is_empty() {
            debug!(
                "Path {} is also owned by: {}. Skipping deletion.",
                file.path,
                other_owners.join(", ")
            );
            continue;
        }

        let full_path = root_dir.join(file.path.trim_start_matches('/'));
        match file.file_type {
            FileType::File | FileType::Symlink => {
                if full_path.exists() || full_path.symlink_metadata().is_ok() {
                    let _ = tokio::fs::remove_file(&full_path).await;
                }
            }
            FileType::Directory => {
                let _ = tokio::fs::remove_dir(&full_path).await;
            }
        }
    }

    phase_start = Instant::now();
    let plan_id = db
        .ensure_plan_registered(
            &partinfo,
            &partinfo.plan.version,
            partinfo.plan.release,
            partinfo.plan.epoch,
            &partinfo.plan.description,
            &partinfo.plan.arch,
            &partinfo.plan.license,
        )
        .await?;
    db.update_part(NewPart {
        name: &partinfo.name,
        plan_id,
        part_hash: Some(part_hash.as_str()),
        install_scripts: hooks_content.as_deref(),
        origin: installed_part.origin, // Preserve origin
    })
    .await?;

    let updated_part = db
        .get_part(&partinfo.name)
        .await?
        .expect("updated package exists");

    for (path, owner_name) in shadows {
        if let Some(owner_part) = db.get_part(&owner_name).await? {
            let diverted_to = if divert_paths.contains(&path) {
                let mut p = PathBuf::from(&path);
                let mut os = p.file_name().unwrap().to_os_string();
                os.push(".wright-diverted");
                p.set_file_name(os);
                Some(p.to_string_lossy().to_string())
            } else {
                None
            };
            let _ = db
                .record_shadowed_file(
                    &path,
                    owner_part.id,
                    updated_part.id,
                    diverted_to.as_deref(),
                )
                .await;
        }
    }

    db.replace_files(updated_part.id, &new_entries).await?;

    let mut deps = Vec::new();
    for d in &partinfo.runtime_deps {
        let (name, constraint) = version::parse_dependency(d).unwrap_or_else(|_| (d.clone(), None));
        deps.push(Dependency {
            name,
            version_constraint: constraint.map(|c| c.to_string()),
        });
    }
    db.replace_dependencies(updated_part.id, &deps).await?;

    self_replace_provides_conflicts(db, updated_part.id, &partinfo).await?;

    tx.commit().await?;
    log_debug_timing(
        "upgrade",
        &partinfo.name,
        "database update",
        phase_start.elapsed(),
    );

    if run_hooks {
        if let Some(ref script) = hooks.post_upgrade {
            log_running_hook(&partinfo.name, "post_upgrade");
            phase_start = Instant::now();
            if let Err(e) =
                run_install_script(script, root_dir, &partinfo.name, "post_upgrade").await
            {
                warn!("post_upgrade script failed: {}", e);
            }
            log_debug_timing(
                "upgrade",
                &partinfo.name,
                "post_upgrade hook",
                phase_start.elapsed(),
            );
        }
    }

    log_debug_timing("upgrade", &partinfo.name, "total", overall_start.elapsed());
    let installed_ver_rel = if installed_plan.version.is_empty() {
        format!("{}", installed_plan.release)
    } else {
        format!("{}-{}", installed_plan.version, installed_plan.release)
    };
    let new_ver_rel = if partinfo.plan.version.is_empty() {
        format!("{}", partinfo.plan.release)
    } else {
        format!("{}-{}", partinfo.plan.version, partinfo.plan.release)
    };
    info!(
        "Upgraded {}: {} -> {}",
        partinfo.name, installed_ver_rel, new_ver_rel,
    );
    Ok(())
}
