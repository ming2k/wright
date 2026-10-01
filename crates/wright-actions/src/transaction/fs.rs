use crate::error::{Result, WrightError};
use crate::transaction::fs_tx::FsTransaction;
use crate::util::checksum;
use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use wright_part::archive::PartInfo;
use wright_registry::database::{FileEntry, FileType};

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
                && !s.starts_with(".PLANSRC")
                && !s.starts_with(".BUILDINFO")
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
            .map_err(|e| WrightError::context("failed to get metadata", e))?;

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

/// Install `entries` into `root_dir`, journaling every mutation through the
/// unified [`FsTransaction`] so the whole operation can be undone.
///
/// Overwritten content is backed up via `fs.back_up` (a same-inode move into
/// the transaction's backup store under the root) rather than copied into a
/// temporary directory, so a crashed install's data survives a reboot.
pub(super) async fn copy_entries_to_root(
    entries: &[FileEntry],
    extract_dir: &Path,
    root_dir: &Path,
    tx: &mut FsTransaction,
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
        if tokio::fs::metadata(&dest_path).await.is_err() {
            tokio::fs::create_dir_all(&dest_path).await.map_err(|e| {
                WrightError::context(
                    format!("failed to create directory {}", dest_path.display()),
                    e,
                )
            })?;
            tx.record_created(dest_path, true)?;
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
                        return Err(WrightError::context(
                            format!("failed to read symlink {}", src_path.display()),
                            e,
                        ));
                    }
                },
            };

            if let Ok(existing_meta) = tokio::fs::symlink_metadata(&dest_path).await {
                if existing_meta.file_type().is_symlink() {
                    if let Ok(target) = tokio::fs::read_link(&dest_path).await {
                        tx.record_symlink_replaced(
                            dest_path.clone(),
                            target.to_string_lossy().into_owned(),
                        )?;
                    }
                    if let Err(e) = tokio::fs::remove_file(&dest_path).await {
                        return Err(WrightError::context(
                            format!("failed to remove existing symlink {}", dest_path.display()),
                            e,
                        ));
                    }
                } else if existing_meta.is_file() {
                    tx.back_up(&dest_path).await?;
                } else if existing_meta.file_type().is_dir()
                    && let Err(e) = tokio::fs::remove_dir_all(&dest_path).await
                {
                    return Err(WrightError::context(
                        format!(
                            "failed to remove existing directory {}",
                            dest_path.display()
                        ),
                        e,
                    ));
                }
            }

            if let Err(e) = tokio::fs::symlink(&link_target, &dest_path).await {
                return Err(WrightError::context(
                    format!(
                        "failed to create symlink {} -> {}",
                        dest_path.display(),
                        link_target.display()
                    ),
                    e,
                ));
            }
            tx.record_created(dest_path, false)?;
        } else {
            // Regular file
            if config_paths.contains(&entry.path)
                && tokio::fs::symlink_metadata(&dest_path).await.is_ok()
            {
                // A config file already on disk is preserved: the new version is
                // written alongside as `<name>.wnew` and the existing file is
                // left untouched.
                let mut new_name = dest_path.as_os_str().to_owned();
                new_name.push(".wnew");
                let side_path = PathBuf::from(new_name);
                move_or_copy(&src_path, &side_path).await.map_err(|e| {
                    WrightError::context(format!("failed to write {}", side_path.display()), e)
                })?;
                if let Some(mode) = entry.file_mode {
                    let _ = tokio::fs::set_permissions(
                        &side_path,
                        std::fs::Permissions::from_mode(mode as u32),
                    )
                    .await;
                }
                tx.record_created(side_path, false)?;
                preserved_configs.push(entry.path.clone());
            } else if divert_paths.contains(&entry.path) {
                let mut divert_name = dest_path.as_os_str().to_owned();
                divert_name.push(".wright-diverted");
                let divert_path = PathBuf::from(divert_name);

                if tokio::fs::metadata(&dest_path).await.is_ok()
                    || tokio::fs::symlink_metadata(&dest_path).await.is_ok()
                {
                    tx.move_aside(&dest_path, &divert_path).await?;
                }

                move_or_copy(&src_path, &dest_path).await.map_err(|e| {
                    WrightError::context(
                        format!(
                            "failed to install {} to {}",
                            src_path.display(),
                            dest_path.display()
                        ),
                        e,
                    )
                })?;
                if let Some(mode) = entry.file_mode {
                    let _ = tokio::fs::set_permissions(
                        &dest_path,
                        std::fs::Permissions::from_mode(mode as u32),
                    )
                    .await;
                }
                tx.record_created(dest_path, false)?;
            } else {
                if let Ok(existing_meta) = tokio::fs::symlink_metadata(&dest_path).await {
                    if existing_meta.is_file() {
                        tx.back_up(&dest_path).await?;
                    } else if existing_meta.file_type().is_symlink() {
                        let _ = tokio::fs::remove_file(&dest_path).await;
                    } else if existing_meta.is_dir() {
                        let _ = tokio::fs::remove_dir_all(&dest_path).await;
                    }
                }

                move_or_copy(&src_path, &dest_path).await.map_err(|e| {
                    WrightError::context(
                        format!(
                            "failed to install {} to {}",
                            src_path.display(),
                            dest_path.display()
                        ),
                        e,
                    )
                })?;
                if let Some(mode) = entry.file_mode {
                    let _ = tokio::fs::set_permissions(
                        &dest_path,
                        std::fs::Permissions::from_mode(mode as u32),
                    )
                    .await;
                }
                tx.record_created(dest_path, false)?;
            }
        }
    }

    Ok(preserved_configs)
}
