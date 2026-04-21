use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use walkdir::WalkDir;

use crate::database::{FileEntry, FileType};
use crate::error::{Result, WrightError};
use crate::part::part::PartInfo;
use crate::transaction::rollback::RollbackState;
use crate::util::checksum;

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

    // Compute per-file SHA-256 in parallel (the only CPU-bound work here).
    let backup_set: HashSet<&str> = partinfo.backup_files.iter().map(|s| s.as_str()).collect();
    let entries: std::result::Result<Vec<FileEntry>, WrightError> =
        raw.par_iter()
            .map(|entry| {
                let relative = entry
                    .path()
                    .strip_prefix(extract_dir)
                    .unwrap_or(entry.path());
                let relative_str = relative.to_string_lossy().to_string();
                let file_path = format!("/{}", relative_str);

                let metadata = entry.path().symlink_metadata().map_err(|e| {
                    WrightError::InstallError(format!("failed to get metadata: {}", e))
                })?;

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

                Ok(FileEntry {
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
                })
            })
            .collect();

    entries
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
fn move_or_copy(src: &Path, dst: &Path) -> std::io::Result<()> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            // If the destination exists, we MUST remove it first because
            // if it's a busy executable, std::fs::copy (which opens for writing)
            // will fail with ETXTBSY. Unlinking it removes the name from
            // the directory, allowing a new file to be created at that name.
            if dst.exists() || dst.symlink_metadata().is_ok() {
                let _ = std::fs::remove_file(dst);
            }
            std::fs::copy(src, dst)?;
            let _ = std::fs::remove_file(src);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Install files from pre-collected [`FileEntry`] records into the target root,
/// avoiding a redundant directory walk when entries have already been gathered
/// by [`collect_file_entries`].
pub(super) fn copy_entries_to_root(
    entries: &[FileEntry],
    extract_dir: &Path,
    root_dir: &Path,
    rollback: &mut RollbackState,
    backup_dir: Option<&Path>,
    config_paths: &HashSet<String>,
    divert_paths: &HashSet<String>,
) -> Result<Vec<String>> {
    // --- Phase 1: create directories (serial, order matters) ---
    for entry in entries {
        if entry.file_type != FileType::Directory {
            continue;
        }
        let relative = entry.path.trim_start_matches('/');
        let dest_path = root_dir.join(relative);
        if !dest_path.exists() {
            std::fs::create_dir_all(&dest_path).map_err(|e| {
                WrightError::InstallError(format!(
                    "failed to create directory {}: {}",
                    dest_path.display(),
                    e
                ))
            })?;
            rollback.record_dir_created(dest_path);
        }
    }

    // --- Phase 2: install files and symlinks in parallel ---

    struct FileResult {
        created: Vec<PathBuf>,
        backup_original: Option<(PathBuf, PathBuf)>,
        symlink_backup: Option<(PathBuf, String)>,
        preserved_config: Option<String>,
        error: Option<WrightError>,
    }

    let file_entries: Vec<_> = entries
        .iter()
        .filter(|e| e.file_type != FileType::Directory)
        .collect();

    let results: Vec<FileResult> = file_entries
        .par_iter()
        .map(|entry| {
            let relative = entry.path.trim_start_matches('/');
            let src_path = extract_dir.join(relative);
            let dest_path = root_dir.join(relative);

            if entry.file_type == FileType::Symlink {
                // --- symlink ---
                let link_target: PathBuf = match entry.file_hash {
                    Some(ref target) => PathBuf::from(target),
                    None => match std::fs::read_link(&src_path) {
                        Ok(t) => t,
                        Err(e) => {
                            return FileResult {
                                created: vec![],
                                backup_original: None,
                                symlink_backup: None,
                                preserved_config: None,
                                error: Some(WrightError::InstallError(format!(
                                    "failed to read symlink {}: {}",
                                    src_path.display(),
                                    e
                                ))),
                            };
                        }
                    },
                };

                let mut symlink_backup = None;
                let mut backup_original = None;

                if let Ok(existing_meta) = dest_path.symlink_metadata() {
                    if existing_meta.file_type().is_symlink() {
                        if let Ok(target) = std::fs::read_link(&dest_path) {
                            symlink_backup =
                                Some((dest_path.clone(), target.to_string_lossy().into_owned()));
                        }
                    } else if existing_meta.is_file() {
                        if let Some(bdir) = backup_dir {
                            let backup_path = bdir.join(relative);
                            if let Some(parent) = backup_path.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            if std::fs::copy(&dest_path, &backup_path).is_ok() {
                                backup_original = Some((dest_path.clone(), backup_path));
                            }
                        }
                    }

                    let remove_result = if existing_meta.file_type().is_dir() {
                        std::fs::remove_dir_all(&dest_path)
                    } else {
                        std::fs::remove_file(&dest_path)
                    };
                    if let Err(e) = remove_result {
                        return FileResult {
                            created: vec![],
                            backup_original,
                            symlink_backup,
                            preserved_config: None,
                            error: Some(WrightError::InstallError(format!(
                                "failed to remove existing file {}: {}",
                                dest_path.display(),
                                e
                            ))),
                        };
                    }
                }

                if let Err(e) = std::os::unix::fs::symlink(&link_target, &dest_path) {
                    return FileResult {
                        created: vec![],
                        backup_original,
                        symlink_backup,
                        preserved_config: None,
                        error: Some(WrightError::InstallError(format!(
                            "failed to create symlink {} -> {}: {}",
                            dest_path.display(),
                            link_target.display(),
                            e
                        ))),
                    };
                }

                FileResult {
                    created: vec![dest_path],
                    backup_original,
                    symlink_backup,
                    preserved_config: None,
                    error: None,
                }
            } else {
                // --- regular file ---
                if config_paths.contains(&entry.path) && dest_path.symlink_metadata().is_ok() {
                    let mut new_name = dest_path.as_os_str().to_owned();
                    new_name.push(".wnew");
                    let side_path = PathBuf::from(new_name);
                    if let Err(e) = move_or_copy(&src_path, &side_path) {
                        return FileResult {
                            created: vec![],
                            backup_original: None,
                            symlink_backup: None,
                            preserved_config: None,
                            error: Some(WrightError::InstallError(format!(
                                "failed to write {}: {}",
                                side_path.display(),
                                e
                            ))),
                        };
                    }
                    if let Some(mode) = entry.file_mode {
                        let _ = std::fs::set_permissions(
                            &side_path,
                            std::fs::Permissions::from_mode(mode),
                        );
                    }
                    FileResult {
                        created: vec![side_path],
                        backup_original: None,
                        symlink_backup: None,
                        preserved_config: Some(entry.path.clone()),
                        error: None,
                    }
                } else if divert_paths.contains(&entry.path) {
                    let mut divert_name = dest_path.as_os_str().to_owned();
                    divert_name.push(".wright-diverted");
                    let divert_path = PathBuf::from(divert_name);

                    let mut backup_original = None;
                    if dest_path.exists() || dest_path.symlink_metadata().is_ok() {
                        if let Err(e) = move_or_copy(&dest_path, &divert_path) {
                            return FileResult {
                                created: vec![],
                                backup_original: None,
                                symlink_backup: None,
                                preserved_config: None,
                                error: Some(WrightError::InstallError(format!(
                                    "failed to divert {} to {}: {}",
                                    dest_path.display(),
                                    divert_path.display(),
                                    e
                                ))),
                            };
                        }
                        backup_original = Some((dest_path.clone(), divert_path));
                    }

                    if let Err(e) = move_or_copy(&src_path, &dest_path) {
                        return FileResult {
                            created: vec![],
                            backup_original,
                            symlink_backup: None,
                            preserved_config: None,
                            error: Some(WrightError::InstallError(format!(
                                "failed to install {} to {}: {}",
                                src_path.display(),
                                dest_path.display(),
                                e
                            ))),
                        };
                    }
                    if let Some(mode) = entry.file_mode {
                        let _ = std::fs::set_permissions(
                            &dest_path,
                            std::fs::Permissions::from_mode(mode),
                        );
                    }
                    FileResult {
                        created: vec![dest_path],
                        backup_original,
                        symlink_backup: None,
                        preserved_config: None,
                        error: None,
                    }
                } else {
                    let mut backup_original = None;
                    if let Some(bdir) = backup_dir {
                        if let Ok(existing_meta) = dest_path.symlink_metadata() {
                            if existing_meta.is_file() {
                                let backup_path = bdir.join(relative);
                                if let Some(parent) = backup_path.parent() {
                                    let _ = std::fs::create_dir_all(parent);
                                }
                                if std::fs::copy(&dest_path, &backup_path).is_ok() {
                                    backup_original = Some((dest_path.clone(), backup_path));
                                }
                            } else if existing_meta.file_type().is_symlink() {
                                let _ = std::fs::remove_file(&dest_path);
                            }
                        }
                    } else if dest_path.exists() {
                        let _ = std::fs::remove_file(&dest_path);
                    }

                    if let Err(e) = move_or_copy(&src_path, &dest_path) {
                        return FileResult {
                            created: vec![],
                            backup_original,
                            symlink_backup: None,
                            preserved_config: None,
                            error: Some(WrightError::InstallError(format!(
                                "failed to install {} to {}: {}",
                                src_path.display(),
                                dest_path.display(),
                                e
                            ))),
                        };
                    }
                    if let Some(mode) = entry.file_mode {
                        let _ = std::fs::set_permissions(
                            &dest_path,
                            std::fs::Permissions::from_mode(mode),
                        );
                    }
                    FileResult {
                        created: vec![dest_path],
                        backup_original,
                        symlink_backup: None,
                        preserved_config: None,
                        error: None,
                    }
                }
            }
        })
        .collect();

    // --- Phase 3: register rollback records (serial) and collect outputs ---
    let mut preserved_configs = Vec::new();
    let mut first_error: Option<WrightError> = None;

    for result in results {
        for path in result.created {
            rollback.record_file_created(path);
        }
        if let Some((original, backup)) = result.backup_original {
            rollback.record_backup(original, backup);
        }
        if let Some((original, target)) = result.symlink_backup {
            rollback.record_symlink_backup(original, target);
        }
        if let Some(cfg) = result.preserved_config {
            preserved_configs.push(cfg);
        }
        if first_error.is_none() {
            first_error = result.error;
        }
    }

    if let Some(e) = first_error {
        return Err(e);
    }

    Ok(preserved_configs)
}
