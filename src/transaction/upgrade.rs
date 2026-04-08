use std::collections::HashSet;
use std::path::Path;

use tracing::{info, warn};

use crate::database::{Database, DepType, Dependency, FileType, NewPart};
use crate::error::{Result, WrightError};
use crate::part::archive;
use crate::part::version::{self, Version};
use crate::transaction::fs::{collect_config_paths, collect_file_entries, copy_files_to_root};
use crate::transaction::hooks::{read_hooks, run_install_script};
use crate::transaction::rollback::RollbackState;

use super::{journal_path_from_db, self_replace_provides_conflicts};

pub fn upgrade_part(
    db: &Database,
    archive_path: &Path,
    root_dir: &Path,
    force: bool,
    run_hooks: bool,
) -> Result<()> {
    let staging_base = archive_path
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or_else(|| std::path::Path::new("/var/lib/wright"));
    let temp_dir = tempfile::Builder::new()
        .prefix("wright-stage-")
        .tempdir_in(staging_base)
        .or_else(|_| tempfile::tempdir())
        .map_err(|e| WrightError::UpgradeError(format!("failed to create temp dir: {}", e)))?;
    let (pkginfo, pkg_hash) = archive::extract_archive(archive_path, temp_dir.path())?;

    let old_pkg = db.get_part(&pkginfo.name)?.ok_or_else(|| {
        WrightError::UpgradeError(format!(
            "part '{}' is not installed, use install instead",
            pkginfo.name
        ))
    })?;

    let old_ver = Version::parse(&old_pkg.version)?;
    let new_ver = Version::parse(&pkginfo.version)?;
    let old_epoch = old_pkg.epoch;
    let new_epoch = pkginfo.epoch;
    if !force {
        let is_newer = if new_epoch != old_epoch {
            new_epoch > old_epoch
        } else if new_ver != old_ver {
            new_ver > old_ver
        } else {
            pkginfo.release > old_pkg.release
        };
        if !is_newer {
            return Err(WrightError::UpgradeError(format!(
                "{} {}-{} is not newer than installed {}-{}",
                pkginfo.name, pkginfo.version, pkginfo.release, old_pkg.version, old_pkg.release,
            )));
        }
    }

    let (hooks_content, hooks) = read_hooks(temp_dir.path());
    let new_entries = collect_file_entries(temp_dir.path(), &pkginfo)?;

    info!(plan = %pkginfo.name, "upgrading: {} files", new_entries.len());

    let file_paths: Vec<&str> = new_entries
        .iter()
        .filter(|e| e.file_type == FileType::File)
        .map(|e| e.path.as_str())
        .collect();
    let owners = db.find_owners_batch(&file_paths)?;
    for entry in &new_entries {
        if entry.file_type == FileType::File {
            if let Some(owner) = owners.get(&entry.path) {
                if *owner != pkginfo.name {
                    if force {
                        warn!(plan = %pkginfo.name, "overwriting {} (owned by {})", entry.path, owner);
                    } else {
                        return Err(WrightError::FileConflict {
                            path: entry.path.clone().into(),
                            owner: owner.clone(),
                        });
                    }
                }
            }
        }
    }

    let tx_id = db.record_transaction(
        "upgrade",
        &pkginfo.name,
        Some(&old_pkg.version),
        Some(&pkginfo.version),
        "pending",
        None,
    )?;

    let mut rollback_state = match journal_path_from_db(db) {
        Some(jp) => RollbackState::with_journal(jp),
        None => RollbackState::new(),
    };

    let old_files = db.get_files(old_pkg.id)?;
    let new_paths: HashSet<&str> = new_entries.iter().map(|e| e.path.as_str()).collect();

    let backup_dir = tempfile::tempdir()
        .map_err(|e| WrightError::UpgradeError(format!("failed to create backup dir: {}", e)))?;

    for old_file in &old_files {
        if !new_paths.contains(old_file.path.as_str()) {
            continue;
        }

        let full_path = root_dir.join(old_file.path.trim_start_matches('/'));
        if !full_path.exists() && full_path.symlink_metadata().is_err() {
            continue;
        }

        if old_file.file_type == FileType::File {
            let backup_path = backup_dir
                .path()
                .join(old_file.path.trim_start_matches('/'));
            if let Some(parent) = backup_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    WrightError::UpgradeError(format!(
                        "failed to create backup directory {}: {}",
                        parent.display(),
                        e
                    ))
                })?;
            }
            std::fs::copy(&full_path, &backup_path).map_err(|e| {
                WrightError::UpgradeError(format!(
                    "failed to backup {}: {}",
                    full_path.display(),
                    e
                ))
            })?;
            rollback_state.record_backup(full_path, backup_path);
        } else if old_file.file_type == FileType::Symlink {
            if let Ok(target) = std::fs::read_link(&full_path) {
                rollback_state
                    .record_symlink_backup(full_path, target.to_string_lossy().to_string());
            }
        }
    }

    if run_hooks {
        if let Some(ref script) = hooks.pre_install {
            info!(plan = %pkginfo.name, "running pre_install hook");
            if let Err(e) = run_install_script(script, root_dir) {
                warn!("pre_install script failed: {}", e);
            }
        }
    }

    let config_paths = collect_config_paths(&new_entries);
    let preserved_configs = match copy_files_to_root(
        temp_dir.path(),
        root_dir,
        &mut rollback_state,
        None,
        &config_paths,
    ) {
        Ok(paths) => paths,
        Err(e) => {
            warn!(plan = %pkginfo.name, "upgrade failed, rolling back: {}", e);
            rollback_state.rollback();
            db.update_transaction_status(tx_id, "rolled_back")?;
            return Err(e);
        }
    };
    for path in preserved_configs {
        info!(plan = %pkginfo.name, "preserved config: {}", path);
    }

    let to_delete_paths: Vec<&str> = old_files
        .iter()
        .filter(|f| !new_paths.contains(f.path.as_str()) && !f.is_config)
        .map(|f| f.path.as_str())
        .collect();
    let other_owners_map = db.get_other_owners_batch(old_pkg.id, &to_delete_paths)?;

    for old_file in old_files.iter().rev() {
        if new_paths.contains(old_file.path.as_str()) {
            continue;
        }

        if old_file.is_config {
            info!(plan = %pkginfo.name, "preserving config file: {}", old_file.path);
            continue;
        }

        let other_owners = other_owners_map
            .get(&old_file.path)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        if !other_owners.is_empty() {
            tracing::debug!(
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
                    let _ = std::fs::remove_file(&full_path);
                }
            }
            FileType::Directory => {
                let _ = std::fs::remove_dir(&full_path);
            }
        }
    }

    db.update_part(NewPart {
        name: &pkginfo.name,
        version: &pkginfo.version,
        release: pkginfo.release,
        epoch: pkginfo.epoch,
        description: &pkginfo.description,
        arch: &pkginfo.arch,
        license: &pkginfo.license,
        url: None,
        install_size: pkginfo.install_size,
        pkg_hash: Some(pkg_hash.as_str()),
        install_scripts: hooks_content.as_deref(),
        ..Default::default()
    })?;

    let updated_pkg = db.get_part(&pkginfo.name)?.expect("updated package exists");
    db.replace_files(updated_pkg.id, &new_entries)?;

    let mut deps = Vec::new();
    for d in &pkginfo.runtime_deps {
        let (name, constraint) = version::parse_dependency(d).unwrap_or_else(|_| (d.clone(), None));
        deps.push(Dependency {
            name,
            constraint: constraint.map(|c| c.to_string()),
            dep_type: DepType::Runtime,
        });
    }
    db.replace_dependencies(updated_pkg.id, &deps)?;
    db.replace_optional_dependencies(updated_pkg.id, &pkginfo.optional_deps)?;

    self_replace_provides_conflicts(db, updated_pkg.id, &pkginfo)?;

    db.update_transaction_status(tx_id, "completed")?;

    if run_hooks {
        if let Some(ref script) = hooks.post_upgrade {
            info!(plan = %pkginfo.name, "running post_upgrade hook");
            if let Err(e) = run_install_script(script, root_dir) {
                warn!("post_upgrade script failed: {}", e);
            }
        }
    }

    rollback_state.commit();

    info!(
        plan = %pkginfo.name,
        "upgraded from {}-{} to {}-{}",
        old_pkg.version, old_pkg.release, pkginfo.version, pkginfo.release,
    );
    Ok(())
}
