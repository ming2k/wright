//! FHS (Filesystem Hierarchy Standard) validation for package staging directories.
//!
//! Wright targets a merged-usr Linux layout. This module validates that all files
//! installed into a package's staging directory (`PKG_DIR`) reside under an
//! allowed FHS path before the archive is created.

use std::fs;
use std::path::Path;
use walkdir::WalkDir;

use crate::error::{Result, WrightError};

/// Validate that every file and symlink in `pkg_dir` resides under an
/// FHS-compliant path for this distribution's merged-usr layout.
///
/// Only files and symlinks are checked — intermediate directories (e.g. `/usr`,
/// `/usr/bin`) are organisational and are implicitly allowed when their contents
/// are allowed.
///
/// Absolute symlink targets are also validated against the same whitelist.
pub fn validate(pkg_dir: &Path, pkg_name: &str) -> Result<()> {
    for entry in WalkDir::new(pkg_dir) {
        let entry = entry.map_err(|e| {
            WrightError::BuildError(format!(
                "failed to walk package directory {}: {}",
                pkg_dir.display(),
                e
            ))
        })?;

        let rel = entry.path().strip_prefix(pkg_dir).unwrap();

        // Skip root and directory entries — only validate files and symlinks.
        if rel.components().count() == 0 || entry.file_type().is_dir() {
            continue;
        }

        let abs = Path::new("/").join(rel);

        if !is_allowed(&abs) {
            let hint = rejection_hint(&abs);
            return Err(WrightError::ValidationError(format!(
                "package '{}': file '{}' violates FHS — {}",
                pkg_name,
                abs.display(),
                hint
            )));
        }

        // For symlinks, also check that absolute targets resolve to an allowed path.
        if entry.path_is_symlink() {
            if let Ok(target) = fs::read_link(entry.path()) {
                if target.is_absolute() && !is_allowed(&target) {
                    let hint = rejection_hint(&target);
                    return Err(WrightError::ValidationError(format!(
                        "package '{}': symlink '{}' points to '{}' which violates FHS — {}",
                        pkg_name,
                        abs.display(),
                        target.display(),
                        hint
                    )));
                }
            }
        }
    }

    Ok(())
}

/// Returns `true` if `path` is under an allowed install prefix for this
/// distribution's merged-usr layout.
///
/// **Allowed prefixes:**
/// - `/usr/{bin,lib,lib64,share,include,libexec,libdata}/`
/// - `/etc/`, `/var/`, `/opt/`, `/boot/`
fn is_allowed(path: &Path) -> bool {
    let mut c = path.components();
    c.next(); // skip RootDir
    match c.next().and_then(|c| c.as_os_str().to_str()) {
        Some("usr") => matches!(
            c.next().and_then(|c| c.as_os_str().to_str()),
            Some("bin" | "lib" | "lib64" | "share" | "include" | "libexec" | "libdata")
        ),
        Some("etc" | "var" | "opt" | "boot") => true,
        _ => false,
    }
}

/// Returns a human-readable hint explaining why `path` was rejected and
/// what the correct destination should be.
fn rejection_hint(path: &Path) -> &'static str {
    let mut c = path.components();
    c.next(); // skip RootDir
    let first = c.next().and_then(|c| c.as_os_str().to_str()).unwrap_or("");
    let second = c.next().and_then(|c| c.as_os_str().to_str()).unwrap_or("");

    match first {
        "bin" | "sbin" => "install to /usr/bin",
        "lib" => "install to /usr/lib",
        "lib64" => "install to /usr/lib or /usr/lib64",
        "home" | "root" => "user data, not for package files",
        "tmp" | "run" => "runtime-only; create via install scripts",
        "usr" => match second {
            "sbin" => "install to /usr/bin",
            "local" => "packages install to /usr directly, not /usr/local",
            _ => "not an FHS-compliant path",
        },
        _ => "not an FHS-compliant path",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_file(dir: &Path, rel_path: &str) {
        let full = dir.join(rel_path.trim_start_matches('/'));
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(&full, b"test").unwrap();
    }

    fn make_symlink(dir: &Path, rel_link: &str, target: &str) {
        let full = dir.join(rel_link.trim_start_matches('/'));
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, &full).unwrap();
    }

    #[test]
    fn test_allowed_usr_bin() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "usr/bin/hello");
        assert!(validate(tmp.path(), "hello").is_ok());
    }

    #[test]
    fn test_allowed_usr_lib() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "usr/lib/libfoo.so.1");
        assert!(validate(tmp.path(), "libfoo").is_ok());
    }

    #[test]
    fn test_allowed_usr_lib64() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "usr/lib64/libbar.so");
        assert!(validate(tmp.path(), "libbar").is_ok());
    }

    #[test]
    fn test_allowed_usr_share() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "usr/share/doc/hello/README");
        assert!(validate(tmp.path(), "hello").is_ok());
    }

    #[test]
    fn test_allowed_etc() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "etc/nginx/nginx.conf");
        assert!(validate(tmp.path(), "nginx").is_ok());
    }

    #[test]
    fn test_allowed_var() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "var/lib/foo/data");
        assert!(validate(tmp.path(), "foo").is_ok());
    }

    #[test]
    fn test_rejected_bin() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "bin/foo");
        let err = validate(tmp.path(), "foo").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("violates FHS"), "expected FHS error, got: {}", msg);
        assert!(msg.contains("install to /usr/bin"), "expected hint, got: {}", msg);
    }

    #[test]
    fn test_rejected_sbin() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "sbin/foo");
        let err = validate(tmp.path(), "foo").unwrap_err();
        assert!(err.to_string().contains("install to /usr/bin"));
    }

    #[test]
    fn test_rejected_usr_sbin() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "usr/sbin/foo");
        let err = validate(tmp.path(), "foo").unwrap_err();
        assert!(err.to_string().contains("install to /usr/bin"));
    }

    #[test]
    fn test_rejected_lib() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "lib/libfoo.so");
        let err = validate(tmp.path(), "foo").unwrap_err();
        assert!(err.to_string().contains("install to /usr/lib"));
    }

    #[test]
    fn test_rejected_lib64() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "lib64/libfoo.so");
        let err = validate(tmp.path(), "foo").unwrap_err();
        assert!(err.to_string().contains("install to /usr/lib"));
    }

    #[test]
    fn test_rejected_usr_local() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "usr/local/bin/foo");
        let err = validate(tmp.path(), "foo").unwrap_err();
        assert!(err.to_string().contains("not /usr/local"));
    }

    #[test]
    fn test_rejected_home() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "home/user/file");
        let err = validate(tmp.path(), "foo").unwrap_err();
        assert!(err.to_string().contains("user data"));
    }

    #[test]
    fn test_rejected_tmp() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "tmp/foo");
        let err = validate(tmp.path(), "foo").unwrap_err();
        assert!(err.to_string().contains("runtime-only"));
    }

    #[test]
    fn test_rejected_run() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "run/foo.pid");
        let err = validate(tmp.path(), "foo").unwrap_err();
        assert!(err.to_string().contains("runtime-only"));
    }

    #[test]
    fn test_rejected_random_path() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "mnt/foo/bar");
        let err = validate(tmp.path(), "foo").unwrap_err();
        assert!(err.to_string().contains("not an FHS-compliant path"));
    }

    #[test]
    fn test_absolute_symlink_rejected_target() {
        let tmp = TempDir::new().unwrap();
        // Create a symlink at a valid path but pointing to an invalid absolute target.
        make_symlink(tmp.path(), "usr/lib/libfoo.so", "/lib/libfoo.so.1");
        let err = validate(tmp.path(), "foo").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("symlink"), "expected symlink error, got: {}", msg);
        assert!(msg.contains("install to /usr/lib"), "expected hint, got: {}", msg);
    }

    #[test]
    fn test_relative_symlink_target_not_checked() {
        let tmp = TempDir::new().unwrap();
        // Relative symlink targets are not checked (they're relative to install prefix).
        make_symlink(tmp.path(), "usr/lib/libfoo.so", "libfoo.so.1");
        assert!(validate(tmp.path(), "foo").is_ok());
    }

    #[test]
    fn test_absolute_symlink_allowed_target() {
        let tmp = TempDir::new().unwrap();
        make_symlink(tmp.path(), "usr/lib/libfoo.so", "/usr/lib/libfoo.so.1");
        assert!(validate(tmp.path(), "foo").is_ok());
    }

    #[test]
    fn test_empty_pkg_dir() {
        let tmp = TempDir::new().unwrap();
        assert!(validate(tmp.path(), "empty").is_ok());
    }

    #[test]
    fn test_mixed_valid_and_invalid() {
        let tmp = TempDir::new().unwrap();
        make_file(tmp.path(), "usr/bin/good");
        make_file(tmp.path(), "bin/bad");
        // Should fail due to the bad file.
        assert!(validate(tmp.path(), "mixed").is_err());
    }
}
