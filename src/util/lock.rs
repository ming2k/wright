use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::error::{Result, WrightError};

#[derive(Debug)]
pub struct ProcessLock {
    _file: File,
    path: PathBuf,
}

impl ProcessLock {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    Shared,
    Exclusive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockIdentity<'a> {
    Command(&'a str),
    Database(&'a str),
}

impl<'a> LockIdentity<'a> {
    pub fn file_name(&self) -> String {
        match self {
            LockIdentity::Command(name) => format!("cmd-{name}.lock"),
            LockIdentity::Database(name) => format!("db-{name}.lock"),
        }
    }
}

pub fn acquire_lock(
    lock_dir: &Path,
    identity: LockIdentity,
    mode: LockMode,
) -> Result<ProcessLock> {
    acquire_lock_with_timeout(lock_dir, identity, mode, Duration::from_secs(30))
}

pub fn acquire_lock_with_timeout(
    lock_dir: &Path,
    identity: LockIdentity,
    mode: LockMode,
    timeout: Duration,
) -> Result<ProcessLock> {
    let lock_path = lock_dir.join(identity.file_name());
    acquire_lock_path_with_timeout(&lock_path, mode, timeout)
}

fn acquire_flock(file: &File, lock_path: &Path, mode: LockMode, timeout: Duration) -> Result<()> {
    let mut delay = Duration::from_millis(50);
    let start = Instant::now();

    let op = match mode {
        LockMode::Shared => libc::LOCK_SH,
        LockMode::Exclusive => libc::LOCK_EX,
    };

    loop {
        let ret = unsafe { libc::flock(file.as_raw_fd(), op | libc::LOCK_NB) };
        if ret == 0 {
            return Ok(());
        }
        if start.elapsed() >= timeout {
            return Err(WrightError::LockError(format!(
                "another wright process is already running (lock held at {})",
                lock_path.display()
            )));
        }
        std::thread::sleep(delay);
        delay = (delay * 2).min(Duration::from_secs(1));
    }
}

/// Helper to derive the standard lock directory from a root path or configuration.
pub fn lock_dir_from_db(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("lock"))
        .unwrap_or_else(|| {
            // Fallback for non-standard paths, though GlobalConfig should ideally provide this
            db_path.parent().unwrap_or(Path::new(".")).join("lock")
        })
}

fn acquire_lock_path_with_timeout(
    lock_path: &Path,
    mode: LockMode,
    timeout: Duration,
) -> Result<ProcessLock> {
    if let Some(parent) = lock_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                return Err(WrightError::AccessDenied(format!(
                    "cannot create directory {}",
                    parent.display()
                )));
            }
            return Err(WrightError::LockError(format!(
                "failed to create lock directory {}: {}",
                parent.display(),
                e
            )));
        }
    }

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                WrightError::AccessDenied(format!("permission denied for lock file {}", lock_path.display()))
            } else {
                WrightError::LockError(format!(
                    "failed to open lock file {}: {}",
                    lock_path.display(),
                    e
                ))
            }
        })?;

    acquire_flock(&file, lock_path, mode, timeout)?;

    if mode == LockMode::Exclusive {
        // Use a more robust write pattern for PID
        let f = file;
        if let Err(e) = f
            .set_len(0)
            .and_then(|_| writeln!(&f, "pid={}", std::process::id()))
        {
            // Non-fatal, but we log it if we had a logger here.
            // For now, we continue since the flock itself is the source of truth.
            eprintln!(
                "warning: failed to write PID to {}: {}",
                lock_path.display(),
                e
            );
        }
        Ok(ProcessLock {
            _file: f,
            path: lock_path.to_path_buf(),
        })
    } else {
        Ok(ProcessLock {
            _file: file,
            path: lock_path.to_path_buf(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{acquire_lock_with_timeout, lock_dir_from_db, LockIdentity, LockMode};

    #[test]
    fn lock_dir_derivation() {
        let db_path = std::path::Path::new("/var/lib/wright/state/installed.db");
        assert_eq!(
            lock_dir_from_db(db_path),
            std::path::Path::new("/var/lib/wright/lock")
        );
    }

    #[test]
    fn named_lock_is_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let lock_dir = dir.path().join("lock");

        let identity = LockIdentity::Command("build");
        let _lock = acquire_lock_with_timeout(
            &lock_dir,
            identity.clone(),
            LockMode::Exclusive,
            Duration::from_millis(100),
        )
        .unwrap();
        let err = acquire_lock_with_timeout(
            &lock_dir,
            identity,
            LockMode::Exclusive,
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(format!("{err}").contains("already holding"));
    }

    #[test]
    fn shared_locks_can_coexist() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("state").join("installed.db");

        let identity = LockIdentity::Database("installed.db");
        let _lock1 = acquire_lock_with_timeout(
            &db_path,
            identity.clone(),
            LockMode::Shared,
            Duration::from_millis(100),
        )
        .unwrap();
        let _lock2 = acquire_lock_with_timeout(
            &db_path,
            identity,
            LockMode::Shared,
            Duration::from_millis(100),
        )
        .unwrap();
    }

    #[test]
    fn exclusive_lock_blocks_shared_locks() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("state").join("installed.db");

        let identity = LockIdentity::Database("installed.db");
        let _lock = acquire_lock_with_timeout(
            &db_path,
            identity.clone(),
            LockMode::Exclusive,
            Duration::from_millis(100),
        )
        .unwrap();
        let err = acquire_lock_with_timeout(
            &db_path,
            identity,
            LockMode::Shared,
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(format!("{err}").contains("already holding"));
    }
}
