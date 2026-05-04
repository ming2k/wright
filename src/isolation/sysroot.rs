use std::ffi::CString;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use crate::error::{Result, WrightError};

const SYSTEM_DIRS: &[&str] = &["/usr", "/bin", "/sbin", "/lib", "/lib64"];

const ETC_FILES: &[&str] = &[
    "/etc/passwd",
    "/etc/group",
    "/etc/hosts",
    "/etc/resolv.conf",
    "/etc/ld.so.conf",
    "/etc/ld.so.cache",
];

/// RAII exclusive flock guard. Lock is released when this is dropped.
struct FlockGuard(std::fs::File);

impl FlockGuard {
    fn acquire(path: &Path) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|e| {
                WrightError::IsolationError(format!("open sysroot lock {}: {e}", path.display()))
            })?;
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if ret != 0 {
            return Err(WrightError::IsolationError(format!(
                "acquire sysroot lock: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(FlockGuard(file))
    }
}

impl Drop for FlockGuard {
    fn drop(&mut self) {
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// Returns true if `rel` (relative to a SYSTEM_DIR root) should be excluded
/// from the sysroot copy: `/usr/local` and `/usr/share/{doc,man,info}`.
fn is_excluded(rel: &Path) -> bool {
    let mut it = rel.components();
    let first = match it.next() {
        Some(c) => c.as_os_str().to_string_lossy().into_owned(),
        None => return false,
    };
    if first == "local" {
        return true;
    }
    if first == "share" {
        return matches!(
            it.next()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .as_deref(),
            Some("doc") | Some("man") | Some("info")
        );
    }
    false
}

pub struct SysrootManager {
    cache_dir: PathBuf,
    source_root: PathBuf,
}

impl SysrootManager {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir,
            source_root: PathBuf::from("/"),
        }
    }

    #[cfg(test)]
    pub fn with_source_root(cache_dir: PathBuf, source_root: PathBuf) -> Self {
        Self {
            cache_dir,
            source_root,
        }
    }

    /// Return the cached sysroot path, building it if stale.
    ///
    /// Concurrency-safe: multiple threads/processes may call this simultaneously;
    /// only one will perform the copy while the others wait, then reuse the result.
    pub fn ensure(&self) -> Result<PathBuf> {
        let sysroot = self.cache_dir.join("sysroot");
        let stamp = self.cache_dir.join("sysroot.stamp");

        if self.is_fresh(&sysroot, &stamp) {
            debug!("Sysroot cache is fresh: {}", sysroot.display());
            return Ok(sysroot);
        }

        std::fs::create_dir_all(&self.cache_dir).map_err(|e| {
            WrightError::IsolationError(format!(
                "create sysroot cache dir {}: {e}",
                self.cache_dir.display()
            ))
        })?;

        let _guard = FlockGuard::acquire(&self.cache_dir.join("sysroot.lock"))?;

        // Re-check after acquiring the lock — another process may have built it.
        if self.is_fresh(&sysroot, &stamp) {
            return Ok(sysroot);
        }

        info!("Building sysroot cache at {}", sysroot.display());
        self.build(&sysroot)?;
        self.write_stamp(&stamp)?;
        info!("Sysroot cache ready");

        Ok(sysroot)
    }

    /// Force a rebuild of the sysroot cache.
    pub fn rebuild(&self) -> Result<PathBuf> {
        let sysroot = self.cache_dir.join("sysroot");
        let stamp = self.cache_dir.join("sysroot.stamp");

        std::fs::create_dir_all(&self.cache_dir).map_err(|e| {
            WrightError::IsolationError(format!(
                "create sysroot cache dir {}: {e}",
                self.cache_dir.display()
            ))
        })?;

        let _guard = FlockGuard::acquire(&self.cache_dir.join("sysroot.lock"))?;

        info!("Rebuilding sysroot cache at {}", sysroot.display());
        self.build(&sysroot)?;
        self.write_stamp(&stamp)?;
        info!("Sysroot cache ready");

        Ok(sysroot)
    }

    fn is_fresh(&self, sysroot: &Path, stamp: &Path) -> bool {
        if !sysroot.is_dir() || !stamp.is_file() {
            return false;
        }

        let stamp_mtime = match std::fs::metadata(stamp).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => return false,
        };

        for dir in SYSTEM_DIRS {
            let path = self.source_root.join(dir.trim_start_matches('/'));
            if !path.exists() {
                continue;
            }
            let mtime = match std::fs::metadata(&path).and_then(|m| m.modified()) {
                Ok(t) => t,
                Err(_) => return false,
            };
            if mtime > stamp_mtime {
                debug!("Sysroot stale: {} changed after stamp", path.display());
                return false;
            }
        }
        true
    }

    fn build(&self, sysroot: &Path) -> Result<()> {
        // Build into a tmp dir; atomic rename at the end prevents partial sysroots.
        let tmp = self.cache_dir.join("sysroot.tmp");
        if tmp.exists() {
            if let Err(e) = std::fs::remove_dir_all(&tmp) {
                warn!("Failed to remove old sysroot.tmp: {e}");
            }
        }
        std::fs::create_dir_all(&tmp).map_err(|e| {
            WrightError::IsolationError(format!("create sysroot.tmp {}: {e}", tmp.display()))
        })?;

        for dir in SYSTEM_DIRS {
            let src = self.source_root.join(dir.trim_start_matches('/'));
            if !src.exists() {
                continue;
            }
            let dest = tmp.join(dir.trim_start_matches('/'));
            self.copy_tree(&src, &dest)?;
        }

        let etc_dest = tmp.join("etc");
        std::fs::create_dir_all(&etc_dest)
            .map_err(|e| WrightError::IsolationError(format!("create sysroot etc: {e}")))?;
        for file in ETC_FILES {
            let src = self.source_root.join(file.trim_start_matches('/'));
            if src.exists() {
                let name = src.file_name().unwrap_or_default();
                if let Err(e) = copy_with_metadata(&src, &etc_dest.join(name)) {
                    warn!("Failed to copy {} to sysroot: {e}", src.display());
                }
            }
        }

        make_readonly_recursive(&tmp)
            .map_err(|e| WrightError::IsolationError(format!("make sysroot read-only: {e}")))?;

        if sysroot.exists() {
            if let Err(e) = std::fs::remove_dir_all(sysroot) {
                warn!("Failed to remove old sysroot: {e}");
            }
        }
        std::fs::rename(&tmp, sysroot).map_err(|e| {
            WrightError::IsolationError(format!(
                "rename sysroot.tmp → sysroot {}: {e}",
                sysroot.display()
            ))
        })?;

        Ok(())
    }

    fn copy_tree(&self, src: &Path, dest: &Path) -> Result<()> {
        for entry in walkdir::WalkDir::new(src)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                // filter_entry(false) skips the entry AND its entire subtree.
                let Ok(rel) = e.path().strip_prefix(src) else {
                    return true;
                };
                !is_excluded(rel)
            })
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            let rel = match path.strip_prefix(src) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let target = if rel.as_os_str().is_empty() || rel.as_os_str() == "." {
                dest.to_path_buf()
            } else {
                dest.join(rel)
            };
            let ft = entry.file_type();

            if ft.is_dir() {
                std::fs::create_dir_all(&target).map_err(|e| {
                    WrightError::IsolationError(format!("create dir {}: {e}", target.display()))
                })?;
            } else if ft.is_symlink() {
                let link_target = std::fs::read_link(path).map_err(|e| {
                    WrightError::IsolationError(format!("read symlink {}: {e}", path.display()))
                })?;
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        WrightError::IsolationError(format!(
                            "create symlink parent {}: {e}",
                            parent.display()
                        ))
                    })?;
                }
                if target.symlink_metadata().is_ok() {
                    let _ = std::fs::remove_file(&target);
                }
                std::os::unix::fs::symlink(&link_target, &target).map_err(|e| {
                    WrightError::IsolationError(format!(
                        "create symlink {} → {}: {e}",
                        target.display(),
                        link_target.display()
                    ))
                })?;
            } else if ft.is_file() {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        WrightError::IsolationError(format!(
                            "create file parent {}: {e}",
                            parent.display()
                        ))
                    })?;
                }
                copy_with_metadata(path, &target).map_err(|e| {
                    WrightError::IsolationError(format!(
                        "copy {} → {}: {e}",
                        path.display(),
                        target.display()
                    ))
                })?;
            }
        }
        Ok(())
    }

    fn write_stamp(&self, stamp: &Path) -> Result<()> {
        std::fs::write(stamp, b"").map_err(|e| {
            WrightError::IsolationError(format!("write sysroot stamp {}: {e}", stamp.display()))
        })
    }
}

#[allow(clippy::permissions_set_readonly_false)]
fn copy_with_metadata(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::copy(src, dest)?;

    let meta = std::fs::metadata(src)?;
    let mut perms = meta.permissions();
    perms.set_readonly(false);
    std::fs::set_permissions(dest, perms)?;

    let accessed = meta.accessed()?;
    let modified = meta.modified()?;
    std::fs::File::open(dest)?.set_times(
        std::fs::FileTimes::new()
            .set_accessed(accessed)
            .set_modified(modified),
    )?;

    // Best-effort ownership preservation (fails gracefully for non-root).
    let uid = meta.uid();
    let gid = meta.gid();
    unsafe {
        if let Ok(c_path) = CString::new(dest.as_os_str().as_encoded_bytes()) {
            libc::chown(c_path.as_ptr(), uid, gid);
        }
    }

    Ok(())
}

fn make_readonly_recursive(path: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        for entry in std::fs::read_dir(path)? {
            make_readonly_recursive(&entry?.path())?;
        }
    }
    let mode = meta.permissions().mode();
    let mut perms = meta.permissions();
    perms.set_mode(mode & !0o222);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

pub fn default_sysroot_cache_dir() -> PathBuf {
    let uid = unsafe { libc::getuid() };
    if uid == 0 {
        PathBuf::from("/var/tmp/wright")
    } else {
        std::env::var("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .ok()
            .or_else(|| {
                std::env::var("HOME")
                    .map(|h| PathBuf::from(h).join(".cache"))
                    .ok()
            })
            .map(|p| p.join("wright"))
            .unwrap_or_else(|| PathBuf::from("/var/tmp/wright"))
    }
}

pub fn ensure_global_sysroot() -> Result<PathBuf> {
    SysrootManager::new(default_sysroot_cache_dir()).ensure()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sysroot_freshness() {
        let tmp = tempfile::tempdir().unwrap();

        let fake_root = tmp.path().join("fakeroot");
        std::fs::create_dir_all(fake_root.join("usr/bin")).unwrap();
        std::fs::create_dir_all(fake_root.join("bin")).unwrap();
        std::fs::create_dir_all(fake_root.join("lib")).unwrap();
        std::fs::create_dir_all(fake_root.join("etc")).unwrap();
        std::fs::write(fake_root.join("usr/bin/true"), b"#!/bin/sh\ntrue\n").unwrap();
        std::fs::write(fake_root.join("bin/sh"), b"#!/bin/sh\n").unwrap();
        std::fs::write(fake_root.join("etc/passwd"), b"").unwrap();

        let mgr = SysrootManager::with_source_root(
            tmp.path().join("cache").to_path_buf(),
            fake_root.clone(),
        );

        assert!(!mgr.is_fresh(
            &tmp.path().join("cache/sysroot"),
            &tmp.path().join("cache/stamp")
        ));

        let sysroot = mgr.ensure().unwrap();
        assert!(sysroot.is_dir());
        assert!(sysroot.join("usr/bin/true").exists());
        assert!(mgr.is_fresh(&sysroot, &tmp.path().join("cache/sysroot.stamp")));

        let meta = std::fs::metadata(sysroot.join("usr/bin/true")).unwrap();
        assert_eq!(meta.permissions().mode() & 0o222, 0);
    }

    #[test]
    fn test_is_excluded() {
        assert!(is_excluded(Path::new("local")));
        assert!(is_excluded(Path::new("local/bin/gcc")));
        assert!(is_excluded(Path::new("share/doc")));
        assert!(is_excluded(Path::new("share/doc/foo/bar.txt")));
        assert!(is_excluded(Path::new("share/man/man1/gcc.1")));
        assert!(is_excluded(Path::new("share/info/gcc.info")));
        assert!(!is_excluded(Path::new("")));
        assert!(!is_excluded(Path::new("share")));
        assert!(!is_excluded(Path::new("share/locale")));
        assert!(!is_excluded(Path::new("lib")));
        assert!(!is_excluded(Path::new("bin")));
    }
}
