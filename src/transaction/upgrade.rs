use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{debug, info, warn};

use crate::database::{DepType, Dependency, FileType, InstalledDb, NewPart};
use crate::error::{Result, WrightError};
use crate::part::part;
use crate::part::version::{self, Version};
use crate::transaction::fs::{collect_config_paths, collect_file_entries, copy_entries_to_root};
use crate::transaction::hooks::{log_running_hook, read_hooks, run_install_script};
use crate::transaction::rollback::RollbackState;

use super::{journal_path_from_db, log_debug_timing, self_replace_provides_conflicts};

pub async fn upgrade_part(
    db: &InstalledDb,
    part_path: &Path,
    root_dir: &Path,
    force: bool,
    run_hooks: bool,
) -> Result<()> {
    let overall_start = Instant::now();

    let staging_dir = std::path::Path::new("/var/lib/wright/staging");
    let _ = tokio::fs::create_dir_all(staging_dir).await;
    let temp_dir = tempfile::tempdir_in(staging_dir)
        .or_else(|_| tempfile::tempdir())
        .map_err(|e| WrightError::UpgradeError(format!("failed to create temp dir: {}", e)))?;
    let mut phase_start = Instant::now();

    // Blocking call to extract_part, but it's mainly I/O.
    // We could wrap it in spawn_blocking if it's too slow.
    let (partinfo, part_hash) = part::extract_part(part_path, temp_dir.path())?;

    log_debug_timing(
        "upgrade",
        &partinfo.name,
        "archive extraction",
        phase_start.elapsed(),
    );

    let old_part = db.get_part(&partinfo.name).await?.ok_or_else(|| {
        WrightError::UpgradeError(format!(
            "part '{}' is not installed, use install instead",
            partinfo.name
        ))
    })?;

    let old_epoch = old_part.epoch as u32;
    let new_epoch = partinfo.epoch;
    if !force {
        let is_newer = if new_epoch != old_epoch {
            new_epoch > old_epoch
        } else {
            match (Version::parse(&old_part.version).ok(), Version::parse(&partinfo.version).ok()) {
                (Some(old_v), Some(new_v)) => {
                    if new_v != old_v {
                        new_v > old_v
                    } else {
                        partinfo.release > old_part.release as u32
                    }
                }
                _ => {
                    // Fallback to string comparison when versions can't be parsed
                    // (e.g., empty versions)
                    let ord = partinfo.version.cmp(&old_part.version);
                    if ord != std::cmp::Ordering::Equal {
                        ord == std::cmp::Ordering::Greater
                    } else {
                        partinfo.release > old_part.release as u32
                    }
                }
            }
        };
        if !is_newer {
            let old_ver_rel = if old_part.version.is_empty() {
                format!("{}", old_part.release)
            } else {
                format!("{}-{}", old_part.version, old_part.release)
            };
            let new_ver_rel = if partinfo.version.is_empty() {
                format!("{}", partinfo.release)
            } else {
                format!("{}-{}", partinfo.version, partinfo.release)
            };
            return Err(WrightError::UpgradeError(format!(
                "{} {} is not newer than installed {}",
                partinfo.name,
                new_ver_rel,
                old_ver_rel,
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
                        super::compact_path(&entry.path),
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

    let tx_id = db
        .record_transaction(
            "upgrade",
            &partinfo.name,
            Some(&old_part.version),
            Some(&partinfo.version),
            "pending",
            None,
        )
        .await?;

    let mut rollback_state = match journal_path_from_db(db) {
        Some(jp) => RollbackState::with_journal(jp),
        None => RollbackState::new(),
    };

    let old_files = db.get_files(old_part.id).await?;
    let new_paths: HashSet<&str> = new_entries.iter().map(|e| e.path.as_str()).collect();

    let backup_dir = tempfile::tempdir()
        .map_err(|e| WrightError::UpgradeError(format!("failed to create backup dir: {}", e)))?;

    // Perform backup. For now, doing it sequentially but with async calls for safety.
    // Parallelizing async I/O can be done with join_all or similar.
    let overlapping: Vec<_> = old_files
        .iter()
        .filter(|f| new_paths.contains(f.path.as_str()))
        .collect();

    for old_file in overlapping {
        let full_path = root_dir.join(old_file.path.trim_start_matches('/'));
        if !full_path.exists() && full_path.symlink_metadata().is_err() {
            continue;
        }

        if old_file.file_type == FileType::File {
            let backup_path = backup_dir
                .path()
                .join(old_file.path.trim_start_matches('/'));
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
                    rollback_state.record_backup(full_path, backup_path);
                }
                Err(e) => {
                    return Err(WrightError::UpgradeError(format!(
                        "failed to backup {}: {}",
                        full_path.display(),
                        e
                    )));
                }
            }
        } else if old_file.file_type == FileType::Symlink {
            if let Ok(target) = tokio::fs::read_link(&full_path).await {
                rollback_state
                    .record_symlink_backup(full_path, target.to_string_lossy().to_string());
            }
        }
    }

    if run_hooks {
        if let Some(ref script) = hooks.pre_install {
            log_running_hook(&partinfo.name, "pre_install");
            phase_start = Instant::now();
            if let Err(e) = run_install_script(script, root_dir).await {
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
        &mut rollback_state,
        None,
        &config_paths,
        &divert_paths,
    )
    .await
    {
        Ok(paths) => paths,
        Err(e) => {
            warn!("Upgrade failed for {}, rolling back: {}", partinfo.name, e);
            rollback_state.rollback();
            db.update_transaction_status(tx_id, "rolled_back").await?;
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

    let to_delete_paths: Vec<&str> = old_files
        .iter()
        .filter(|f| !new_paths.contains(f.path.as_str()) && !f.is_config)
        .map(|f| f.path.as_str())
        .collect();
    let other_owners_map = db
        .get_other_owners_batch(old_part.id, &to_delete_paths)
        .await?;

    for old_file in old_files.iter().rev() {
        if new_paths.contains(old_file.path.as_str()) {
            continue;
        }

        if old_file.is_config {
            info!(
                "Preserving config file for {}: {}",
                partinfo.name, old_file.path
            );
            continue;
        }

        let other_owners = other_owners_map
            .get(&old_file.path)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        if !other_owners.is_empty() {
            debug!(
                "Path {} is also owned by: {}. Skipping deletion.",
                old_file.path,
                other_owners.join(", ")
            );
            continue;
        }

        let full_path = root_dir.join(old_file.path.trim_start_matches('/'));
        match old_file.file_type {
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
    db.update_part(NewPart {
        name: &partinfo.name,
        version: &partinfo.version,
        release: partinfo.release,
        epoch: partinfo.epoch,
        description: &partinfo.description,
        arch: &partinfo.arch,
        license: &partinfo.license,
        url: None,
        install_size: partinfo.install_size,
        part_hash: Some(part_hash.as_str()),
        install_scripts: hooks_content.as_deref(),
        origin: old_part.origin, // Preserve origin
        plan_name: old_part.plan_name.as_deref(), // Preserve plan_name
        plan_id: old_part.plan_id, // Preserve plan_id
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
            dep_type: DepType::Runtime,
        });
    }
    db.replace_dependencies(updated_part.id, &deps).await?;
    db.replace_optional_dependencies(updated_part.id, &partinfo.optional_deps)
        .await?;

    self_replace_provides_conflicts(db, updated_part.id, &partinfo).await?;

    db.update_transaction_status(tx_id, "completed").await?;
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
            if let Err(e) = run_install_script(script, root_dir).await {
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

    rollback_state.commit();

    log_debug_timing("upgrade", &partinfo.name, "total", overall_start.elapsed());
    let old_ver_rel = if old_part.version.is_empty() {
        format!("{}", old_part.release)
    } else {
        format!("{}-{}", old_part.version, old_part.release)
    };
    let new_ver_rel = if partinfo.version.is_empty() {
        format!("{}", partinfo.release)
    } else {
        format!("{}-{}", partinfo.version, partinfo.release)
    };
    info!(
        "Upgraded {}: {} -> {}",
        partinfo.name, old_ver_rel, new_ver_rel,
    );
    Ok(())
}
