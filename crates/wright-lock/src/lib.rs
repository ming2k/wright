//! Advisory process locks shared by commands and database handles.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub mod error;
pub use error::{LockError, Result};

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

pub fn acquire_lock_path_with_timeout(
    lock_path: &Path,
    mode: LockMode,
    timeout: Duration,
) -> Result<ProcessLock> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| LockError::CreateDir {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|e| LockError::OpenFile {
            path: lock_path.to_path_buf(),
            source: e,
        })?;

    let operation = match mode {
        LockMode::Shared => libc::LOCK_SH,
        LockMode::Exclusive => libc::LOCK_EX,
    };

    let start = Instant::now();
    let poll_interval = Duration::from_millis(50);

    loop {
        let ret = unsafe { libc::flock(file.as_raw_fd(), operation | libc::LOCK_NB) };
        if ret == 0 {
            if mode == LockMode::Exclusive {
                let mut file_clone = match file.try_clone() {
                    Ok(f) => f,
                    Err(e) => {
                        return Err(LockError::Io {
                            path: lock_path.to_path_buf(),
                            source: e,
                        });
                    }
                };
                let _ = file_clone.set_len(0);
                let _ = writeln!(file_clone, "pid: {}", std::process::id());
                let _ = file_clone.flush();
            }
            return Ok(ProcessLock {
                _file: file,
                path: lock_path.to_path_buf(),
            });
        }

        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(LockError::Io {
                path: lock_path.to_path_buf(),
                source: err,
            });
        }

        if start.elapsed() >= timeout {
            return Err(LockError::Timeout {
                path: lock_path.to_path_buf(),
                timeout_secs: timeout.as_secs(),
            });
        }

        std::thread::sleep(poll_interval);
    }
}

pub fn lock_dir_from_db(db_path: &Path) -> PathBuf {
    if let Some(parent) = db_path.parent() {
        if parent == Path::new("") {
            PathBuf::from("locks")
        } else {
            parent.join("locks")
        }
    } else {
        PathBuf::from("locks")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lock_acquire_and_release() {
        let tmp = tempfile::tempdir().unwrap();
        let lock_dir = tmp.path();

        let lock1 = acquire_lock(lock_dir, LockIdentity::Command("test"), LockMode::Exclusive).unwrap();
        assert!(lock1.path().exists());

        // A second exclusive lock with 100ms timeout must fail with Timeout
        let err = acquire_lock_with_timeout(
            lock_dir,
            LockIdentity::Command("test"),
            LockMode::Exclusive,
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(matches!(err, LockError::Timeout { .. }));

        drop(lock1);

        // Now acquire should succeed immediately
        let lock2 = acquire_lock(lock_dir, LockIdentity::Command("test"), LockMode::Exclusive);
        assert!(lock2.is_ok());
    }
}
