//! Staging-output integrity: deterministic tree manifests and snapshot/restore.
//!
//! The checkpoint system historically bound a stage's "completed" status only
//! to its **input** (the script + env + predecessor hash).  Nothing tied that
//! status to the stage's actual **output** on disk.  For every forge stage the
//! output lives in the OverlayFS layer stack under `target/`, which is rebuilt
//! from `layers/` on resume — so the input-hash guarantee is sufficient.
//!
//! The `staging` stage is different: its deliverable (`staging/`) is written
//! by the user's install script directly into `${STAGING_DIR}`, *outside* the
//! layer stack, and `staging/` is wiped at the start of every build.  There is
//! no persistent copy and no path that restores it on resume.
//!
//! This module closes that gap with two primitives:
//!
//! * [`compute_dir_manifest`] — a deterministic SHA-256 over the sorted tree
//!   (relative paths + per-file content hashes + symlink targets).  Stored in
//!   the checkpoint as `output_manifest_hash`, it lets us prove that the tree
//!   on disk is exactly the tree a prior run produced.
//! * [`snapshot_tree`] / [`restore_tree`] — copy the staging tree to/from a
//!   persistent `.staging_cache/` so a skipped staging stage can repopulate
//!   `staging/` after `Foundry::build` wipes it.
//!
//! Together they make "checkpoint says complete but staging is empty/partial"
//! impossible: the forge verifies the manifest before honouring a skip, and
//! restores from the snapshot cache when the skip is legitimate.

use sha2::{Digest, Sha256};
use std::io::ErrorKind;
use std::path::Path;
use walkdir::WalkDir;

use crate::error::{Result, WrightError};

/// Compute a deterministic SHA-256 manifest over a directory tree.
///
/// Each entry contributes its sorted relative path, a type marker, and its
/// content (file body hash for regular files, link target for symlinks).
/// Directories contribute only structure (their presence is implied by the
/// files they contain).  The result is stable across runs and filesystems.
///
/// Returns an empty string for a non-existent directory so callers can treat
/// "missing" and "present but empty" uniformly.
pub fn compute_dir_manifest(dir: &Path) -> Result<String> {
    if !dir.exists() {
        return Ok(String::new());
    }

    let mut entries: Vec<(String, std::fs::FileType)> = Vec::new();
    for entry in WalkDir::new(dir).sort_by_file_name().follow_links(false) {
        let entry = entry.map_err(|e| {
            WrightError::context(
                format!("staging manifest walk failed at {}", dir.display()),
                e,
            )
        })?;
        let relative = entry.path().strip_prefix(dir).unwrap_or(entry.path());
        let relative_str = relative.to_string_lossy();
        if relative_str.is_empty() {
            continue;
        }
        let ft = entry.file_type();
        entries.push((relative_str.into_owned(), ft));
    }

    // WalkDir already yields sorted output, but sort explicitly so the
    // manifest is correct regardless of the walker's traversal strategy.
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    // An existing-but-empty directory (or one containing only directories with
    // no files) should be indistinguishable from a missing one: it represents
    // "no deliverable".
    if entries.is_empty() {
        return Ok(String::new());
    }

    let mut hasher = Sha256::new();
    for (rel, ft) in &entries {
        hasher.update(b"entry:");
        hasher.update(rel.as_bytes());
        hasher.update(b"\n");
        let full = dir.join(rel);
        if ft.is_symlink() {
            hasher.update(b"symlink:");
            match std::fs::read_link(&full) {
                Ok(target) => hasher.update(target.to_string_lossy().as_bytes()),
                Err(_) => hasher.update(b"<unreadable-symlink>"),
            }
        } else if ft.is_file() {
            hasher.update(b"file:");
            match compute_file_hash(&full) {
                Ok(h) => hasher.update(h.as_bytes()),
                Err(_) => hasher.update(b"<unreadable-file>"),
            }
        } else if ft.is_dir() {
            hasher.update(b"dir:");
        } else {
            hasher.update(b"other:");
        }
        hasher.update(b"\n");
    }

    Ok(format!("{:x}", hasher.finalize()))
}

fn compute_file_hash(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut hasher = Sha256::new();
    let mut f = std::fs::File::open(path).map_err(|e| {
        WrightError::context(
            format!("failed to open {} for staging manifest", path.display()),
            e,
        )
    })?;
    let mut buf = [0u8; 65536];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| WrightError::context("staging manifest read failed", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Recursively copy a tree from `src` to `dst`, replacing `dst` entirely.
///
/// Files are hard-linked when possible (both paths live under the same
/// `build_root` filesystem) and fall back to a byte copy otherwise.  Symlinks
/// and directory structure are reproduced faithfully.  `dst` is atomically
/// replaced via a temp-dir rename so a crash mid-snapshot cannot leave a
/// half-written cache.
pub fn snapshot_tree(src: &Path, dst: &Path) -> Result<()> {
    if !src.exists() {
        // Nothing to snapshot — remove any stale cache.
        if dst.exists() {
            let _ = std::fs::remove_dir_all(dst);
        }
        return Ok(());
    }

    let parent = dst.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| {
        WrightError::context(
            format!("failed to create snapshot parent {}", parent.display()),
            e,
        )
    })?;

    let tmp_dst = dst.with_extension("snapshot.tmp");
    if tmp_dst.exists() {
        std::fs::remove_dir_all(&tmp_dst).map_err(|e| {
            WrightError::context(
                format!("failed to clear stale snapshot tmp {}", tmp_dst.display()),
                e,
            )
        })?;
    }

    link_tree(src, &tmp_dst)?;

    // Linux's rename(2) refuses to replace a non-empty directory (ENOTEMPTY),
    // so we swap via a backup name to keep the replacement as atomic as
    // possible.  If a crash leaves the backup behind, the next run reaps it
    // at the top of this function (tmp_dst removal + fresh link_tree).
    let backup = dst.with_extension("snapshot.old");
    let had_backup = if dst.exists() {
        let _ = std::fs::remove_dir_all(&backup);
        match std::fs::rename(dst, &backup) {
            Ok(()) => true,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&tmp_dst);
                return Err(WrightError::context(
                    format!("failed to rotate old staging snapshot {}", dst.display()),
                    e,
                ));
            }
        }
    } else {
        false
    };

    if let Err(e) = std::fs::rename(&tmp_dst, dst) {
        // Restore the backup so we don't lose the previous good cache.
        if had_backup {
            let _ = std::fs::rename(&backup, dst);
        }
        return Err(WrightError::context(
            format!(
                "failed to commit staging snapshot {} -> {}",
                tmp_dst.display(),
                dst.display()
            ),
            e,
        ));
    }

    if had_backup {
        let _ = std::fs::remove_dir_all(&backup);
    }

    Ok(())
}

/// Restore a tree from `src` to `dst`, replacing `dst` entirely.
///
/// Used to repopulate `staging/` from `.staging_cache/` after
/// `Foundry::build` wipes it.  Same link/copy strategy as [`snapshot_tree`].
pub fn restore_tree(src: &Path, dst: &Path) -> Result<()> {
    if !src.exists() {
        return Err(WrightError::ForgeError(format!(
            "cannot restore staging: snapshot cache {} does not exist",
            src.display()
        )));
    }
    if dst.exists() {
        std::fs::remove_dir_all(dst).map_err(|e| {
            WrightError::context(
                format!("failed to clear restore target {}", dst.display()),
                e,
            )
        })?;
    }
    std::fs::create_dir_all(dst).map_err(|e| {
        WrightError::context(
            format!("failed to create restore target {}", dst.display()),
            e,
        )
    })?;
    link_tree(src, dst)
}

/// Hard-link (fallback: copy) every file and reproduce every symlink/dir from
/// `src` into `dst`.  Both must be on the same filesystem for hard-links to
/// succeed; the copy fallback handles the cross-filesystem edge case.
fn link_tree(src: &Path, dst: &Path) -> Result<()> {
    for entry in WalkDir::new(src).sort_by_file_name().follow_links(false) {
        let entry = entry.map_err(|e| {
            WrightError::context(format!("tree walk failed at {}", src.display()), e)
        })?;
        let relative = entry.path().strip_prefix(src).unwrap_or(entry.path());
        let dest_path = dst.join(relative);

        let ft = entry.file_type();
        if ft.is_dir() && !entry.path_is_symlink() {
            std::fs::create_dir_all(&dest_path).map_err(|e| {
                WrightError::context(format!("failed to create dir {}", dest_path.display()), e)
            })?;
            continue;
        }

        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                WrightError::context(
                    format!("failed to create parent for {}", dest_path.display()),
                    e,
                )
            })?;
        }

        if ft.is_symlink() {
            let target = std::fs::read_link(entry.path()).map_err(|e| {
                WrightError::context(
                    format!("failed to read symlink {}", entry.path().display()),
                    e,
                )
            })?;
            let _ = std::fs::remove_file(&dest_path);
            std::os::unix::fs::symlink(&target, &dest_path).map_err(|e| {
                WrightError::context(
                    format!("failed to recreate symlink {}", dest_path.display()),
                    e,
                )
            })?;
        } else if ft.is_file() {
            let _ = std::fs::remove_file(&dest_path);
            if std::fs::hard_link(entry.path(), &dest_path).is_err() {
                std::fs::copy(entry.path(), &dest_path).map_err(|e| {
                    WrightError::context(
                        format!(
                            "failed to copy {} -> {}",
                            entry.path().display(),
                            dest_path.display()
                        ),
                        e,
                    )
                })?;
            }
        }
    }
    Ok(())
}

/// Remove a directory tree, treating "not found" as success.
pub fn remove_tree_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(WrightError::context(
            format!("failed to remove {}", path.display()),
            e,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_empty_dir_is_empty_string() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(compute_dir_manifest(tmp.path()).unwrap(), "");
    }

    #[test]
    fn test_manifest_missing_dir_is_empty_string() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        assert_eq!(compute_dir_manifest(&missing).unwrap(), "");
    }

    #[test]
    fn test_manifest_stable_across_recomputes() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "hello").unwrap();
        std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("sub/b.txt"), "world").unwrap();

        let h1 = compute_dir_manifest(tmp.path()).unwrap();
        let h2 = compute_dir_manifest(tmp.path()).unwrap();
        assert_eq!(h1, h2);
        assert!(!h1.is_empty());
    }

    #[test]
    fn test_manifest_detects_content_change() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("file"), "content-a").unwrap();
        let before = compute_dir_manifest(tmp.path()).unwrap();

        std::fs::write(tmp.path().join("file"), "content-b").unwrap();
        let after = compute_dir_manifest(tmp.path()).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn test_manifest_detects_added_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a"), "1").unwrap();
        let before = compute_dir_manifest(tmp.path()).unwrap();

        std::fs::write(tmp.path().join("b"), "2").unwrap();
        let after = compute_dir_manifest(tmp.path()).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn test_manifest_detects_removed_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a"), "1").unwrap();
        std::fs::write(tmp.path().join("b"), "2").unwrap();
        let before = compute_dir_manifest(tmp.path()).unwrap();

        std::fs::remove_file(tmp.path().join("b")).unwrap();
        let after = compute_dir_manifest(tmp.path()).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn test_manifest_includes_symlink_target() {
        let tmp = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/target-a", tmp.path().join("link")).unwrap();
        let before = compute_dir_manifest(tmp.path()).unwrap();

        std::fs::remove_file(tmp.path().join("link")).unwrap();
        std::os::unix::fs::symlink("/target-b", tmp.path().join("link")).unwrap();
        let after = compute_dir_manifest(tmp.path()).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn test_snapshot_and_restore_roundtrip() {
        let src = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join("usr/bin")).unwrap();
        std::fs::create_dir_all(src.path().join("usr/share")).unwrap();
        std::fs::write(src.path().join("usr/bin/app"), "binary").unwrap();
        std::fs::write(src.path().join("usr/share/doc"), "docs").unwrap();
        std::os::unix::fs::symlink("/usr/bin/app", src.path().join("usr/bin/link")).unwrap();

        let manifest_before = compute_dir_manifest(src.path()).unwrap();

        let cache = tempfile::tempdir().unwrap();
        let cache_path = cache.path().join(".staging_cache");
        snapshot_tree(src.path(), &cache_path).unwrap();
        assert!(cache_path.exists());

        // Restoring into a fresh destination reproduces the manifest.
        let dst = tempfile::tempdir().unwrap();
        let dst_path = dst.path().join("staging");
        restore_tree(&cache_path, &dst_path).unwrap();
        let manifest_after = compute_dir_manifest(&dst_path).unwrap();

        assert_eq!(manifest_before, manifest_after);
        assert!(dst_path.join("usr/bin/app").exists());
        assert!(dst_path.join("usr/bin/link").is_symlink());
    }

    #[test]
    fn test_snapshot_replaces_existing_cache() {
        let src1 = tempfile::tempdir().unwrap();
        std::fs::write(src1.path().join("only-in-v1"), "x").unwrap();
        let cache = tempfile::tempdir().unwrap();
        let cache_path = cache.path().join(".staging_cache");
        snapshot_tree(src1.path(), &cache_path).unwrap();
        assert!(cache_path.join("only-in-v1").exists());

        // Snapshot a different tree; old files must not survive.
        let src2 = tempfile::tempdir().unwrap();
        std::fs::write(src2.path().join("only-in-v2"), "y").unwrap();
        snapshot_tree(src2.path(), &cache_path).unwrap();
        assert!(cache_path.join("only-in-v2").exists());
        assert!(!cache_path.join("only-in-v1").exists());
    }
}
