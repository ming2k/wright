use std::collections::HashSet;
use std::path::Path;

use tracing::{debug, info, warn};

use crate::database::{Database, DepType, Dependency, FileType, NewPart};
use crate::error::{Result, WrightError};
use crate::part::archive;
use crate::part::version::{self, Version};
use crate::transaction::fs::{collect_config_paths, collect_file_entries, copy_files_to_root};
use crate::transaction::hooks::{read_hooks, run_install_script};
use crate::transaction::rollback::RollbackState;
use crate::util::checksum;

use super::{journal_path_from_db, self_replace_provides_conflicts};

pub fn upgrade_part(
    db: &Database,
    archive_path: &Path,
    root_dir: &Path,
    force: bool,
    run_hooks: bool,
) -> Result<()> {
    let temp_dir = tempfile::tempdir()
        .map_err(|e| WrightError::UpgradeError(format!("failed to create temp dir: {}", e)))?;
    let pkginfo = archive::extract_archive(archive_path, temp_dir.path())?;

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

    info!(
        "Upgrading {}: {} files",
        pkginfo.name,
        new_entries.len()
    );

    for entry in &new_entries {
        if entry.file_type == FileType::File {
            if let Some(owner) = db.find_owner(&entry.path)? {
                if owner != pkginfo.name {
                    if force {
                        warn!(
                            "{}: overwriting {} (owned by {})",
                            pkginfo.name, entry.path, owner
                        );
                    } else {
                        return Err(WrightError::FileConflict {
                            path: entry.path.clone().into(),
                            owner,
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
            info!("Running pre_install hook for {} (upgrade)", pkginfo.name);
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
            warn!("Upgrade failed, rolling back: {}", e);
            rollback_state.rollback();
            db.update_transaction_status(tx_id, "rolled_back")?;
            return Err(e);
        }
    };
    for path in preserved_configs {
        info!("Preserved config: {} (.wnew: {}.wnew)", path, path);
    }

    for old_file in old_files.iter().rev() {
        if new_paths.contains(old_file.path.as_str()) {
            continue;
        }

        if old_file.is_config {
            info!("Preserving config file: {}", old_file.path);
            continue;
        }

        let other_owners = db.get_other_owners(old_pkg.id, &old_file.path)?;
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

    let pkg_hash = checksum::sha256_file(archive_path).ok();
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
        pkg_hash: pkg_hash.as_deref(),
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
            info!("Running post_upgrade hook for {}", pkginfo.name);
            if let Err(e) = run_install_script(script, root_dir) {
                warn!("post_upgrade script failed: {}", e);
            }
        }
    }

    rollback_state.commit();

    info!(
        "Upgraded {} from {}-{} to {}-{}",
        pkginfo.name, old_pkg.version, old_pkg.release, pkginfo.version, pkginfo.release,
    );
    Ok(())
}
