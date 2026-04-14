use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rayon::prelude::*;
use tracing::{debug, info, warn};

use crate::database::{Database, DepType, Dependency, FileType, NewPart};
use crate::error::{Result, WrightError};
use crate::part::part;
use crate::part::version::{self, Version};
use crate::transaction::fs::{collect_config_paths, collect_file_entries, copy_entries_to_root};
use crate::transaction::hooks::{log_running_hook, read_hooks, run_install_script};
use crate::transaction::rollback::RollbackState;

use super::{journal_path_from_db, log_debug_timing, self_replace_provides_conflicts};

pub fn upgrade_part(
    db: &Database,
    part_path: &Path,
    root_dir: &Path,
    force: bool,
    run_hooks: bool,
) -> Result<()> {
    let overall_start = Instant::now();

    let staging_dir = std::path::Path::new("/var/lib/wright/staging");
    let _ = std::fs::create_dir_all(staging_dir);
    let temp_dir = tempfile::tempdir_in(staging_dir)
        .or_else(|_| tempfile::tempdir())
        .map_err(|e| WrightError::UpgradeError(format!("failed to create temp dir: {}", e)))?;
    let mut phase_start = Instant::now();
    let (pkginfo, pkg_hash) = part::extract_part(part_path, temp_dir.path())?;
    log_debug_timing(
        "upgrade",
        &pkginfo.name,
        "archive extraction",
        phase_start.elapsed(),
    );

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
    phase_start = Instant::now();
    let new_entries = collect_file_entries(temp_dir.path(), &pkginfo)?;
    log_debug_timing(
        "upgrade",
        &pkginfo.name,
        "file scan and metadata collection",
        phase_start.elapsed(),
    );

    info!("Upgrading {}: {} files", pkginfo.name, new_entries.len());

    phase_start = Instant::now();
    let file_paths: Vec<&str> = new_entries
        .iter()
        .filter(|e| e.file_type == FileType::File)
        .map(|e| e.path.as_str())
        .collect();
    let owners = db.find_owners_batch(&file_paths)?;
    let mut shadows = Vec::new();
    let mut divert_paths = HashSet::new();
    for entry in &new_entries {
        if entry.file_type == FileType::File {
            if let Some(owner) = owners.get(&entry.path) {
                if *owner != pkginfo.name {
                    warn!(
                        "[{}] diverted {} (owned by {})",
                        pkginfo.name,
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
        &pkginfo.name,
        "owner conflict check",
        phase_start.elapsed(),
    );

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

    // Back up overlapping files in parallel, using hard links where possible.
    let overlapping: Vec<_> = old_files
        .iter()
        .filter(|f| new_paths.contains(f.path.as_str()))
        .collect();

    struct BackupResult {
        file_backup: Option<(PathBuf, PathBuf)>,
        symlink_backup: Option<(PathBuf, String)>,
        error: Option<WrightError>,
    }

    let backup_results: Vec<BackupResult> = overlapping
        .par_iter()
        .map(|old_file| {
            let full_path = root_dir.join(old_file.path.trim_start_matches('/'));
            if !full_path.exists() && full_path.symlink_metadata().is_err() {
                return BackupResult {
                    file_backup: None,
                    symlink_backup: None,
                    error: None,
                };
            }

            if old_file.file_type == FileType::File {
                let backup_path = backup_dir
                    .path()
                    .join(old_file.path.trim_start_matches('/'));
                if let Some(parent) = backup_path.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        return BackupResult {
                            file_backup: None,
                            symlink_backup: None,
                            error: Some(WrightError::UpgradeError(format!(
                                "failed to create backup directory {}: {}",
                                parent.display(),
                                e
                            ))),
                        };
                    }
                }
                // Prefer hard_link (instant, no data copy) with copy fallback.
                let result = std::fs::hard_link(&full_path, &backup_path)
                    .or_else(|_| std::fs::copy(&full_path, &backup_path).map(|_| ()));
                match result {
                    Ok(()) => BackupResult {
                        file_backup: Some((full_path, backup_path)),
                        symlink_backup: None,
                        error: None,
                    },
                    Err(e) => BackupResult {
                        file_backup: None,
                        symlink_backup: None,
                        error: Some(WrightError::UpgradeError(format!(
                            "failed to backup {}: {}",
                            full_path.display(),
                            e
                        ))),
                    },
                }
            } else if old_file.file_type == FileType::Symlink {
                if let Ok(target) = std::fs::read_link(&full_path) {
                    BackupResult {
                        file_backup: None,
                        symlink_backup: Some((full_path, target.to_string_lossy().to_string())),
                        error: None,
                    }
                } else {
                    BackupResult {
                        file_backup: None,
                        symlink_backup: None,
                        error: None,
                    }
                }
            } else {
                BackupResult {
                    file_backup: None,
                    symlink_backup: None,
                    error: None,
                }
            }
        })
        .collect();

    for result in backup_results {
        if let Some(e) = result.error {
            return Err(e);
        }
        if let Some((original, backup)) = result.file_backup {
            rollback_state.record_backup(original, backup);
        }
        if let Some((original, target)) = result.symlink_backup {
            rollback_state.record_symlink_backup(original, target);
        }
    }

    if run_hooks {
        if let Some(ref script) = hooks.pre_install {
            log_running_hook(&pkginfo.name, "pre_install");
            phase_start = Instant::now();
            if let Err(e) = run_install_script(script, root_dir) {
                warn!("pre_install script failed: {}", e);
            }
            log_debug_timing(
                "upgrade",
                &pkginfo.name,
                "pre_install hook",
                phase_start.elapsed(),
            );
        }
    }

    let config_paths = collect_config_paths(&new_entries);
    phase_start = Instant::now();
    let preserved_configs = match copy_entries_to_root(
        &new_entries,
        temp_dir.path(),
        root_dir,
        &mut rollback_state,
        None,
        &config_paths,
        &divert_paths,
    ) {
        Ok(paths) => paths,
        Err(e) => {
            warn!("Upgrade failed for {}, rolling back: {}", pkginfo.name, e);
            rollback_state.rollback();
            db.update_transaction_status(tx_id, "rolled_back")?;
            return Err(e);
        }
    };
    log_debug_timing(
        "upgrade",
        &pkginfo.name,
        "filesystem copy into target root",
        phase_start.elapsed(),
    );
    for path in preserved_configs {
        info!("Preserved config for {}: {}", pkginfo.name, path);
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
            info!(
                "Preserving config file for {}: {}",
                pkginfo.name, old_file.path
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
                    let _ = std::fs::remove_file(&full_path);
                }
            }
            FileType::Directory => {
                let _ = std::fs::remove_dir(&full_path);
            }
        }
    }

    phase_start = Instant::now();
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

    for (path, owner_name) in shadows {
        if let Some(owner_pkg) = db.get_part(&owner_name)? {
            let diverted_to = if divert_paths.contains(&path) {
                let mut p = PathBuf::from(&path);
                let mut os = p.file_name().unwrap().to_os_string();
                os.push(".wright-diverted");
                p.set_file_name(os);
                Some(p.to_string_lossy().to_string())
            } else {
                None
            };
            let _ = db.record_shadowed_file(
                &path,
                owner_pkg.id,
                updated_pkg.id,
                diverted_to.as_deref(),
            );
        }
    }

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
    log_debug_timing(
        "upgrade",
        &pkginfo.name,
        "database update",
        phase_start.elapsed(),
    );

    if run_hooks {
        if let Some(ref script) = hooks.post_upgrade {
            log_running_hook(&pkginfo.name, "post_upgrade");
            phase_start = Instant::now();
            if let Err(e) = run_install_script(script, root_dir) {
                warn!("post_upgrade script failed: {}", e);
            }
            log_debug_timing(
                "upgrade",
                &pkginfo.name,
                "post_upgrade hook",
                phase_start.elapsed(),
            );
        }
    }

    rollback_state.commit();

    log_debug_timing("upgrade", &pkginfo.name, "total", overall_start.elapsed());
    info!(
        "Upgraded {}: {}-{} -> {}-{}",
        pkginfo.name, old_pkg.version, old_pkg.release, pkginfo.version, pkginfo.release,
    );
    Ok(())
}
