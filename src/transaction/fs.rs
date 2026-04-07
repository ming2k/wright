use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tracing::warn;
use walkdir::WalkDir;

use crate::database::{FileEntry, FileType};
use crate::error::{Result, WrightError};
use crate::part::archive::PartInfo;
use crate::transaction::rollback::RollbackState;
use crate::util::checksum;

pub(super) fn collect_file_entries(
    extract_dir: &Path,
    pkginfo: &PartInfo,
) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::new();

    for entry in WalkDir::new(extract_dir)
        .follow_links(false)
        .sort_by_file_name()
    {
        let entry = entry
            .map_err(|e| WrightError::InstallError(format!("failed to walk directory: {}", e)))?;

        let relative = entry
            .path()
            .strip_prefix(extract_dir)
            .unwrap_or(entry.path());
        let relative_str = relative.to_string_lossy().to_string();

        if relative_str.is_empty()
            || relative_str.starts_with(".PARTINFO")
            || relative_str.starts_with(".FILELIST")
            || relative_str.starts_with(".HOOKS")
        {
            continue;
        }

        let file_path = format!("/{}", relative_str);
        let metadata = entry
            .path()
            .symlink_metadata()
            .map_err(|e| WrightError::InstallError(format!("failed to get metadata: {}", e)))?;

        let file_type = if metadata.is_dir() {
            FileType::Directory
        } else if metadata.file_type().is_symlink() {
            FileType::Symlink
        } else {
            FileType::File
        };

        let file_hash = match file_type {
            FileType::File => checksum::sha256_file(entry.path()).ok(),
            FileType::Symlink => std::fs::read_link(entry.path())
                .ok()
                .map(|t| t.to_string_lossy().to_string()),
            FileType::Directory => None,
        };

        let is_config = pkginfo.backup_files.iter().any(|f| f == &file_path);

        entries.push(FileEntry {
            path: file_path,
            file_hash,
            file_size: if file_type == FileType::File {
                Some(metadata.len())
            } else {
                None
            },
            file_type,
            file_mode: Some(metadata.permissions().mode()),
            is_config,
        });
    }

    Ok(entries)
}

fn backup_existing_path(
    dest_path: &Path,
    relative_str: &str,
    backup_root: &Path,
    rollback: &mut RollbackState,
) -> Result<()> {
    let Ok(existing_meta) = dest_path.symlink_metadata() else {
        return Ok(());
    };
    if existing_meta.file_type().is_symlink() {
        if let Ok(target) = std::fs::read_link(dest_path) {
            rollback
                .record_symlink_backup(dest_path.to_path_buf(), target.to_string_lossy().into());
        }
    } else if existing_meta.is_file() {
        let backup_path = backup_root.join(relative_str);
        if let Some(parent) = backup_path.parent() {
            std::fs::create_dir_all(parent).map_err(WrightError::IoError)?;
        }
        std::fs::copy(dest_path, &backup_path).map_err(WrightError::IoError)?;
        rollback.record_backup(dest_path.to_path_buf(), backup_path);
    }
    Ok(())
}

pub(super) fn collect_config_paths(new_entries: &[FileEntry]) -> HashSet<String> {
    new_entries
        .iter()
        .filter(|e| e.is_config && e.file_type == FileType::File)
        .map(|e| e.path.clone())
        .collect()
}

pub(super) fn copy_files_to_root(
    extract_dir: &Path,
    root_dir: &Path,
    rollback: &mut RollbackState,
    backup_dir: Option<&Path>,
    config_paths: &HashSet<String>,
) -> Result<Vec<String>> {
    let mut preserved_configs = Vec::new();
    for entry in WalkDir::new(extract_dir)
        .follow_links(false)
        .sort_by_file_name()
    {
        let entry = entry
            .map_err(|e| WrightError::InstallError(format!("failed to walk directory: {}", e)))?;

        let relative = entry
            .path()
            .strip_prefix(extract_dir)
            .unwrap_or(entry.path());
        let relative_str = relative.to_string_lossy().to_string();

        if relative_str.is_empty()
            || relative_str.starts_with(".PARTINFO")
            || relative_str.starts_with(".FILELIST")
            || relative_str.starts_with(".HOOKS")
        {
            continue;
        }

        let dest_path = root_dir.join(&relative_str);
        let metadata = entry
            .path()
            .symlink_metadata()
            .map_err(|e| WrightError::InstallError(format!("failed to get metadata: {}", e)))?;

        if metadata.is_dir() {
            if !dest_path.exists() {
                std::fs::create_dir_all(&dest_path).map_err(|e| {
                    WrightError::InstallError(format!(
                        "failed to create directory {}: {}",
                        dest_path.display(),
                        e
                    ))
                })?;
                rollback.record_dir_created(dest_path.clone());
            }
        } else if metadata.file_type().is_symlink() {
            let link_target = std::fs::read_link(entry.path()).map_err(|e| {
                WrightError::InstallError(format!(
                    "failed to read symlink {}: {}",
                    entry.path().display(),
                    e
                ))
            })?;

            if let Some(backup_root) = backup_dir {
                backup_existing_path(&dest_path, &relative_str, backup_root, rollback)?;
            }

            if let Some(parent) = dest_path.parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        WrightError::InstallError(format!(
                            "failed to create directory {}: {}",
                            parent.display(),
                            e
                        ))
                    })?;
                }
            }

            if dest_path.symlink_metadata().is_ok() {
                // dest_path may be a real directory (not a symlink); remove_file
                // fails on directories, so fall back to remove_dir_all.
                let remove_result = if dest_path
                    .symlink_metadata()
                    .map(|m| m.file_type().is_dir())
                    .unwrap_or(false)
                {
                    std::fs::remove_dir_all(&dest_path)
                } else {
                    std::fs::remove_file(&dest_path)
                };
                remove_result.map_err(|e| {
                    WrightError::InstallError(format!(
                        "failed to remove existing file {}: {}",
                        dest_path.display(),
                        e
                    ))
                })?;
            }

            std::os::unix::fs::symlink(&link_target, &dest_path).map_err(|e| {
                WrightError::InstallError(format!(
                    "failed to create symlink {} -> {}: {}",
                    dest_path.display(),
                    link_target.display(),
                    e
                ))
            })?;

            rollback.record_file_created(dest_path);
        } else {
            if let Some(parent) = dest_path.parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        WrightError::InstallError(format!(
                            "failed to create directory {}: {}",
                            parent.display(),
                            e
                        ))
                    })?;
                }
            }

            let canonical_path = format!("/{}", relative_str);
            if config_paths.contains(&canonical_path) && dest_path.symlink_metadata().is_ok() {
                let mut new_name = dest_path.as_os_str().to_owned();
                new_name.push(".wnew");
                let side_path = PathBuf::from(new_name);
                std::fs::copy(entry.path(), &side_path).map_err(|e| {
                    WrightError::InstallError(format!(
                        "failed to write {}: {}",
                        side_path.display(),
                        e
                    ))
                })?;
                if let Err(e) = std::fs::set_permissions(&side_path, metadata.permissions()) {
                    warn!(
                        "Failed to set permissions on {}: {}",
                        side_path.display(),
                        e
                    );
                }
                rollback.record_file_created(side_path);
                preserved_configs.push(canonical_path);
            } else {
                if let Some(backup_root) = backup_dir {
                    backup_existing_path(&dest_path, &relative_str, backup_root, rollback)?;
                }

                if dest_path.exists() {
                    let _ = std::fs::remove_file(&dest_path);
                }
                std::fs::copy(entry.path(), &dest_path).map_err(|e| {
                    WrightError::InstallError(format!(
                        "failed to copy {} to {}: {}",
                        entry.path().display(),
                        dest_path.display(),
                        e
                    ))
                })?;

                if let Err(e) = std::fs::set_permissions(&dest_path, metadata.permissions()) {
                    warn!(
                        "Failed to set permissions on {}: {}",
                        dest_path.display(),
                        e
                    );
                }

                rollback.record_file_created(dest_path);
            }
        }
    }

    Ok(preserved_configs)
}
