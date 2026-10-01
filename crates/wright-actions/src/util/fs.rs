//! Filesystem and permission utilities for public and diagnostic artifacts (ADR-0050).
//!
//! Provides deterministic permission sanitization immune to ambient process umask:
//! - Diagnostic logs and public workspaces are ensured to be world-readable/traversable (0755 / 0644).
//! - Special bits (like sticky bit on `/var/tmp`) and owner rights are preserved via bitwise OR.

use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Default permission for public directories (world traversable and readable: `rwxr-xr-x`).
pub const DIR_PUBLIC_MODE: u32 = 0o755;

/// Default permission for diagnostic log files (world readable: `rw-r--r--`).
pub const FILE_PUBLIC_MODE: u32 = 0o644;

/// Ensure a directory exists and has at least `target_mode` permissions (typically `0o755`).
///
/// Uses bitwise OR (`current_mode | target_mode`) to safely add read and traversal bits
/// without stripping special bits (such as sticky bit `0o1777` on parent tmp directories).
/// Non-fatal when permission changes are not permitted (e.g. unprivileged user traversing
/// host-managed parents).
pub fn ensure_dir_mode<P: AsRef<Path>>(path: P, target_mode: u32) -> std::io::Result<()> {
    let path = path.as_ref();
    std::fs::create_dir_all(path)?;
    relax_path_permissions(path, target_mode);
    Ok(())
}

/// Asynchronous version of `ensure_dir_mode`.
pub async fn ensure_dir_mode_async<P: AsRef<Path>>(
    path: P,
    target_mode: u32,
) -> std::io::Result<()> {
    let path = path.as_ref();
    tokio::fs::create_dir_all(path).await?;
    relax_path_permissions(path, target_mode);
    Ok(())
}

/// Ensure a public directory exists with at least `0o755` permissions.
pub fn ensure_public_dir<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    ensure_dir_mode(path, DIR_PUBLIC_MODE)
}

/// Asynchronous version of `ensure_public_dir`.
pub async fn ensure_public_dir_async<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    ensure_dir_mode_async(path, DIR_PUBLIC_MODE).await
}

/// Ensure `path` and all intermediate parent directories up to `root_limit` (or root)
/// have at least `target_mode` permissions (INV-PERM-01, INV-PERM-03).
///
/// This prevents inner directories from being rendered inaccessible due to restricted
/// parent directories created under a strict umask (e.g. `0077`).
pub fn ensure_public_tree<P: AsRef<Path>>(
    path: P,
    root_limit: Option<&Path>,
) -> std::io::Result<()> {
    let path = path.as_ref();
    std::fs::create_dir_all(path)?;

    #[cfg(unix)]
    {
        let mut curr = Some(path);
        while let Some(p) = curr {
            relax_path_permissions(p, DIR_PUBLIC_MODE);
            if let Some(limit) = root_limit
                && p == limit
            {
                break;
            }
            curr = p.parent();
        }
    }
    Ok(())
}

/// Asynchronous version of `ensure_public_tree`.
pub async fn ensure_public_tree_async<P: AsRef<Path>>(
    path: P,
    root_limit: Option<&Path>,
) -> std::io::Result<()> {
    let path = path.as_ref();
    tokio::fs::create_dir_all(path).await?;

    #[cfg(unix)]
    {
        let mut curr = Some(path);
        while let Some(p) = curr {
            relax_path_permissions(p, DIR_PUBLIC_MODE);
            if let Some(limit) = root_limit
                && p == limit
            {
                break;
            }
            curr = p.parent();
        }
    }
    Ok(())
}

/// Relax permissions on an existing file or directory by bitwise-ORing `target_mode`.
///
/// For example, mode `0o600 | 0o644` becomes `0o644` (`rw-r--r--`).
/// Mode `0o700 | 0o755` becomes `0o755` (`rwxr-xr-x`).
/// Sticky directory `0o1777 | 0o755` remains `0o1777`.
pub fn relax_path_permissions<P: AsRef<Path>>(path: P, target_mode: u32) {
    #[cfg(unix)]
    {
        let p = path.as_ref();
        if let Ok(meta) = std::fs::symlink_metadata(p) {
            let current = meta.permissions().mode();
            let desired = current | target_mode;
            if current != desired {
                let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(desired));
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, target_mode);
    }
}

/// Asynchronous version of `relax_path_permissions`.
pub async fn relax_path_permissions_async<P: AsRef<Path>>(path: P, target_mode: u32) {
    relax_path_permissions(path, target_mode);
}

/// Explicitly ensure a log file has world-readable permissions (`0644`).
pub fn relax_file_permissions<P: AsRef<Path>>(path: P, target_mode: u32) {
    relax_path_permissions(path, target_mode);
}

/// Asynchronous version of `relax_file_permissions`.
pub async fn relax_file_permissions_async<P: AsRef<Path>>(path: P, target_mode: u32) {
    relax_path_permissions_async(path, target_mode).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    #[cfg(unix)]
    fn test_ensure_public_dir_relaxes_strict_umask() {
        let tmp = tempdir().unwrap();
        let sub = tmp.path().join("a/b/c");

        // Manually simulate strict directory creation (0700)
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            std::fs::metadata(&sub).unwrap().permissions().mode() & 0o777,
            0o700
        );

        // Relax with ensure_public_dir
        ensure_public_dir(&sub).unwrap();
        assert_eq!(
            std::fs::metadata(&sub).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_ensure_public_tree_relaxes_all_ancestors() {
        let tmp = tempdir().unwrap();
        let pkg_dir = tmp.path().join("workshop/emacs-31.1");
        let logs_dir = pkg_dir.join("logs");

        std::fs::create_dir_all(&logs_dir).unwrap();
        std::fs::set_permissions(&pkg_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&logs_dir, std::fs::Permissions::from_mode(0o700)).unwrap();

        // Relax tree up to tmp
        ensure_public_tree(&logs_dir, Some(tmp.path())).unwrap();

        assert_eq!(
            std::fs::metadata(&pkg_dir).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            std::fs::metadata(&logs_dir).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_relax_file_permissions() {
        let tmp = tempdir().unwrap();
        let log_file = tmp.path().join("compile.log");

        std::fs::write(&log_file, "compiling...").unwrap();
        std::fs::set_permissions(&log_file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            std::fs::metadata(&log_file).unwrap().permissions().mode() & 0o777,
            0o600
        );

        relax_file_permissions(&log_file, FILE_PUBLIC_MODE);
        assert_eq!(
            std::fs::metadata(&log_file).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_preserves_special_bits() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("sticky_dir");
        std::fs::create_dir(&dir).unwrap();
        // Set sticky bit + 0700
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o1700)).unwrap();

        relax_path_permissions(&dir, DIR_PUBLIC_MODE);
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o1755);
    }
}
