use crate::database::{FileEntry, FileType};
use crate::error::{Result, WrightError};
use crate::part::part::PartInfo;
use crate::transaction::rollback::RollbackState;
use crate::util::checksum;
use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub(super) fn collect_file_entries(
    extract_dir: &Path,
    partinfo: &PartInfo,
) -> Result<Vec<FileEntry>> {
    // Collect paths first (serial, preserves deterministic order).
    let raw: Vec<_> = WalkDir::new(extract_dir)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let rel = e.path().strip_prefix(extract_dir).unwrap_or(e.path());
            let s = rel.to_string_lossy();
            !s.is_empty()
                && !s.starts_with(".PARTINFO")
                && !s.starts_with(".FILELIST")
                && !s.starts_with(".HOOKS")
        })
        .collect();

    // Still using serial or rayon for checksums because it's CPU bound.
    // For now, let's keep it simple.
    let backup_set: HashSet<&str> = partinfo.backup_files.iter().map(|s| s.as_str()).collect();
    let mut entries = Vec::new();

    for entry in raw {
        let relative = entry
            .path()
            .strip_prefix(extract_dir)
            .unwrap_or(entry.path());
        let relative_str = relative.to_string_lossy().to_string();
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

        let is_config = backup_set.contains(file_path.as_str());

        entries.push(FileEntry {
            path: file_path,
            file_hash,
            file_size: if file_type == FileType::File {
                Some(metadata.len() as i64)
            } else {
                None
            },
            file_type,
            file_mode: Some(metadata.permissions().mode() as i64),
            is_config,
        });
    }

    Ok(entries)
}

pub(super) fn collect_config_paths(new_entries: &[FileEntry]) -> HashSet<String> {
    new_entries
        .iter()
        .filter(|e| e.is_config && e.file_type == FileType::File)
        .map(|e| e.path.clone())
        .collect()
}

/// Move `src` to `dst`, using rename(2) when possible (same filesystem) and
/// falling back to copy+delete when crossing filesystem boundaries (EXDEV).
async fn move_or_copy(src: &Path, dst: &Path) -> std::io::Result<()> {
    match tokio::fs::rename(src, dst).await {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            if tokio::fs::metadata(dst).await.is_ok()
                || tokio::fs::symlink_metadata(dst).await.is_ok()
            {
                let _ = tokio::fs::remove_file(dst).await;
            }
            tokio::fs::copy(src, dst).await?;
            let _ = tokio::fs::remove_file(src).await;
            Ok(())
        }
        Err(e) => Err(e),
    }
}

pub(super) async fn copy_entries_to_root(
    entries: &[FileEntry],
    extract_dir: &Path,
    root_dir: &Path,
    rollback: &mut RollbackState,
    backup_dir: Option<&Path>,
    config_paths: &HashSet<String>,
    divert_paths: &HashSet<String>,
) -> Result<Vec<String>> {
    // --- Phase 1: create directories ---
    for entry in entries {
        if entry.file_type != FileType::Directory {
            continue;
        }
        let relative = entry.path.trim_start_matches('/');
        let dest_path = root_dir.join(relative);
        if !tokio::fs::metadata(&dest_path).await.is_ok() {
            tokio::fs::create_dir_all(&dest_path).await.map_err(|e| {
                WrightError::InstallError(format!(
                    "failed to create directory {}: {}",
                    dest_path.display(),
                    e
                ))
            })?;
            rollback.record_dir_created(dest_path);
        }
    }

    // --- Phase 2: install files and symlinks ---
    let mut preserved_configs = Vec::new();

    for entry in entries {
        if entry.file_type == FileType::Directory {
            continue;
        }

        let relative = entry.path.trim_start_matches('/');
        let src_path = extract_dir.join(relative);
        let dest_path = root_dir.join(relative);

        if entry.file_type == FileType::Symlink {
            let link_target: PathBuf = match entry.file_hash {
                Some(ref target) => PathBuf::from(target),
                None => match tokio::fs::read_link(&src_path).await {
                    Ok(t) => t,
                    Err(e) => {
                        return Err(WrightError::InstallError(format!(
                            "failed to read symlink {}: {}",
                            src_path.display(),
                            e
                        )));
                    }
                },
            };

            if let Ok(existing_meta) = tokio::fs::symlink_metadata(&dest_path).await {
                if existing_meta.file_type().is_symlink() {
                    if let Ok(target) = tokio::fs::read_link(&dest_path).await {
                        rollback.record_symlink_backup(
                            dest_path.clone(),
                            target.to_string_lossy().into_owned(),
                        );
                    }
                } else if existing_meta.is_file() {
                    if let Some(bdir) = backup_dir {
                        let backup_path = bdir.join(relative);
                        if let Some(parent) = backup_path.parent() {
                            let _ = tokio::fs::create_dir_all(parent).await;
                        }
                        if tokio::fs::copy(&dest_path, &backup_path).await.is_ok() {
                            rollback.record_backup(dest_path.clone(), backup_path);
                        }
                    }
                }

                let remove_result = if existing_meta.file_type().is_dir() {
                    tokio::fs::remove_dir_all(&dest_path).await
                } else {
                    tokio::fs::remove_file(&dest_path).await
                };
                if let Err(e) = remove_result {
                    return Err(WrightError::InstallError(format!(
                        "failed to remove existing file {}: {}",
                        dest_path.display(),
                        e
                    )));
                }
            }

            if let Err(e) = tokio::fs::symlink(&link_target, &dest_path).await {
                return Err(WrightError::InstallError(format!(
                    "failed to create symlink {} -> {}: {}",
                    dest_path.display(),
                    link_target.display(),
                    e
                )));
            }
            rollback.record_file_created(dest_path);
        } else {
            // Regular file
            if config_paths.contains(&entry.path)
                && tokio::fs::symlink_metadata(&dest_path).await.is_ok()
            {
                let mut new_name = dest_path.as_os_str().to_owned();
                new_name.push(".wnew");
                let side_path = PathBuf::from(new_name);
                move_or_copy(&src_path, &side_path).await.map_err(|e| {
                    WrightError::InstallError(format!(
                        "failed to write {}: {}",
                        side_path.display(),
                        e
                    ))
                })?;
                if let Some(mode) = entry.file_mode {
                    let _ = tokio::fs::set_permissions(
                        &side_path,
                        std::fs::Permissions::from_mode(mode as u32),
                    )
                    .await;
                }
                rollback.record_file_created(side_path);
                preserved_configs.push(entry.path.clone());
            } else if divert_paths.contains(&entry.path) {
                let mut divert_name = dest_path.as_os_str().to_owned();
                divert_name.push(".wright-diverted");
                let divert_path = PathBuf::from(divert_name);

                if tokio::fs::metadata(&dest_path).await.is_ok()
                    || tokio::fs::symlink_metadata(&dest_path).await.is_ok()
                {
                    move_or_copy(&dest_path, &divert_path).await.map_err(|e| {
                        WrightError::InstallError(format!(
                            "failed to divert {} to {}: {}",
                            dest_path.display(),
                            divert_path.display(),
                            e
                        ))
                    })?;
                    rollback.record_backup(dest_path.clone(), divert_path);
                }

                move_or_copy(&src_path, &dest_path).await.map_err(|e| {
                    WrightError::InstallError(format!(
                        "failed to install {} to {}: {}",
                        src_path.display(),
                        dest_path.display(),
                        e
                    ))
                })?;
                if let Some(mode) = entry.file_mode {
                    let _ = tokio::fs::set_permissions(
                        &dest_path,
                        std::fs::Permissions::from_mode(mode as u32),
                    )
                    .await;
                }
                rollback.record_file_created(dest_path);
            } else {
                if let Some(bdir) = backup_dir {
                    if let Ok(existing_meta) = tokio::fs::symlink_metadata(&dest_path).await {
                        if existing_meta.is_file() {
                            let backup_path = bdir.join(relative);
                            if let Some(parent) = backup_path.parent() {
                                let _ = tokio::fs::create_dir_all(parent).await;
                            }
                            if tokio::fs::copy(&dest_path, &backup_path).await.is_ok() {
                                rollback.record_backup(dest_path.clone(), backup_path);
                            }
                        } else if existing_meta.file_type().is_symlink() {
                            let _ = tokio::fs::remove_file(&dest_path).await;
                        }
                    }
                } else if tokio::fs::metadata(&dest_path).await.is_ok() {
                    let _ = tokio::fs::remove_file(&dest_path).await;
                }

                move_or_copy(&src_path, &dest_path).await.map_err(|e| {
                    WrightError::InstallError(format!(
                        "failed to install {} to {}: {}",
                        src_path.display(),
                        dest_path.display(),
                        e
                    ))
                })?;
                if let Some(mode) = entry.file_mode {
                    let _ = tokio::fs::set_permissions(
                        &dest_path,
                        std::fs::Permissions::from_mode(mode as u32),
                    )
                    .await;
                }
                rollback.record_file_created(dest_path);
            }
        }
    }

    Ok(preserved_configs)
}
